//! The v2 driver against a scripted host and remote: the order of a cycle over several feeds (personal
//! first, then the co-members' logs, then each shared feed: membership, push, snapshots, pull, marks,
//! chunks, head), the way each refusal of the server is told to the core, the transfer budget per feed,
//! a feed that fails alone, the session, the backoff and the notifications. Tokio's clock is paused, so
//! every delay is exact. The whole stack (real cores, a real server) is in `tests/fleet.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kb_crypto::Signature;
use kb_sync::v2::conformance::{Person, Space};
use kb_sync::v2::{
    FeedChunkKind, FeedChunkWork, FeedCreated, FeedListing, FeedWork, InlineEntry, InlineOutcome,
    InviteOpened, LinkOutcome, LinkPut, LinkWork, LogChunk, LogRequest, MarksPage, MembershipHead,
    MembershipPage, OutboundEntry, ServerInfo, SnapshotMark, SnapshotsApplied, SyncApplied,
};
use kb_sync::{ChangesPage, PairingPage, SnapshotsPage, SyncError};
use kb_types::sync::{
    FeedKind, PROTOCOL_VERSION, PULL_INTERVAL_MS, PUSH_DEBOUNCE_MS, TRANSFER_BUDGET,
};
use kb_types::{
    AccountId, ApiError, AttachmentId, ChunkId, DeviceId, DocId, Event, FeedId, InviteId, LinkId,
    SyncInfo, SyncState,
};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{RemoteFuture, SyncDriver, SyncHost, SyncRemote};
use crate::RecoveryUpload;
use crate::driver::{
    BACKOFF_MAX, BACKOFF_MIN, Backoff, DriverHandle, IDLE_DEVICE, INVITE_TOKEN_REFUSED,
    MAX_DEBOUNCE, NO_KIT, QUOTA_FULL, RemoteFactory,
};
use crate::http::AuthSigner;
use crate::realtime::Realtime;

mod links;
mod realtime;

const URL: &str = "https://sync.example.test";
const OTHER_URL: &str = "https://other.example.test";
const EPOCH: [u8; 16] = [5; 16];

type Log = Arc<Mutex<Vec<String>>>;
type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

fn account() -> AccountId {
    AccountId::from_bytes([1; 16])
}

fn device() -> DeviceId {
    DeviceId::from_bytes([2; 16])
}

/// The personal feed: its id is the account id.
fn personal() -> FeedId {
    FeedId::from_bytes([1; 16])
}

fn shared_a() -> FeedId {
    FeedId::from_bytes([2; 16])
}

fn shared_b() -> FeedId {
    FeedId::from_bytes([3; 16])
}

fn doc(byte: u8) -> DocId {
    DocId::from_bytes([byte; 16])
}

fn chunk(byte: u8) -> ChunkId {
    ChunkId::from_bytes([byte; 32])
}

fn work(feed: FeedId, kind: FeedKind, cursor: u64) -> FeedWork {
    FeedWork {
        feed,
        kind,
        space_id: None,
        cursor,
        closed: false,
        forked: false,
    }
}

fn personal_work() -> FeedWork {
    work(personal(), FeedKind::Personal, 0)
}

fn shared_work(feed: FeedId) -> FeedWork {
    work(feed, FeedKind::Shared, 0)
}

fn tag(feed: FeedId) -> u8 {
    feed.as_bytes()[0]
}

fn outbound(byte: u8, seq: u64) -> OutboundEntry {
    OutboundEntry {
        doc_id: doc(byte),
        device_seq: seq,
        bytes: vec![byte; 8],
        up_to_seq: i64::from(byte),
    }
}

fn page(cursors: &[u64], more: bool) -> ChangesPage {
    ChangesPage {
        server_epoch: EPOCH,
        changes: cursors.iter().map(|c| (*c, vec![*c as u8; 4])).collect(),
        chain_hash_after: [0; 32],
        more,
    }
}

fn chunk_item(feed: FeedId, byte: u8, kind: FeedChunkKind) -> FeedChunkWork {
    FeedChunkWork {
        feed,
        hash: chunk(byte),
        attachment_id: AttachmentId::from_bytes([byte; 16]),
        kind,
    }
}

/// A `Create` (seq 0) and a `Profile` (seq 1) of one space: real membership entries, which the driver
/// tells apart by their `seq`.
fn membership_entries() -> (Vec<u8>, Vec<u8>) {
    let person = Person::generate(11, "Ada", 0);
    let mut space = Space::new(12, "Project", 0);
    let create = space.create(&person, 1);
    space.accept(&create);
    let profile = space.profile(&person, "Ada");
    (create.encode(), profile.encode())
}

// ----- the fake host ---------------------------------------------------------------------------

struct HostData {
    configured: bool,
    server_url: String,
    pending_entries: u32,
    parked_items: u32,
    fork_detected: bool,
    identity: Option<(AccountId, DeviceId)>,
    log: Vec<Vec<u8>>,
    published: u64,
    /// What `sync_feeds` answers, call after call (the last list stays).
    feed_lists: VecDeque<Vec<FeedWork>>,
    outbox: HashMap<FeedId, Vec<OutboundEntry>>,
    membership: HashMap<FeedId, Vec<Vec<u8>>>,
    wanted: HashMap<FeedId, VecDeque<Option<u64>>>,
    candidates: HashMap<FeedId, Vec<DocId>>,
    /// What `sync_feed_apply` answers for a feed, in order (then the page's last cursor).
    applied: HashMap<FeedId, VecDeque<SyncApplied>>,
    cursors: HashMap<FeedId, u64>,
    pages_applied: Vec<(FeedId, Vec<u64>)>,
    snapshots_applied: HashMap<FeedId, VecDeque<SnapshotsApplied>>,
    marks_wanted: HashMap<FeedId, VecDeque<Option<u64>>>,
    marks_docs: HashMap<FeedId, Vec<DocId>>,
    chunk_work: HashMap<FeedId, Vec<FeedChunkWork>>,
    needed_logs: VecDeque<Vec<LogRequest>>,
    foreign_logs: Vec<AccountId>,
    states: Vec<(SyncState, Option<String>)>,
    recovery_blob: Option<([u8; 32], Vec<u8>)>,
    recovery_obsolete: Vec<[u8; 32]>,
    /// Chunks the core no longer holds (an upload of one is skipped).
    chunk_gone: Vec<ChunkId>,
    /// `sync_remote_epoch` answers `true` (the server's data was reset).
    epoch_reset: bool,
    pushed: Vec<(FeedId, DocId, u64, u64)>,
    posted: Vec<(FeedId, u64)>,
    refused: Vec<FeedId>,
    lost: Vec<FeedId>,
    verified: Vec<(FeedId, DocId, bool)>,
    chunks_told: Vec<(&'static str, FeedId, ChunkId)>,
    fail: HashMap<&'static str, VecDeque<ApiError>>,
    /// Failures of one feed's call.
    fail_feed: HashMap<(&'static str, FeedId), VecDeque<ApiError>>,
    token: Option<String>,
    unknown_asked: usize,
    /// What `sync_feed_apply_inline` answers, in order (then `Pull`, like the trait's default).
    inline_outcomes: VecDeque<InlineOutcome>,
    /// The entries offered to `sync_feed_apply_inline`: the feed, the cursor of the entry.
    inline_calls: Vec<(FeedId, u64)>,
    /// What `sync_push_debounce_ms` answers.
    debounce_ms: u64,
    /// The presence frames handed to `presence_received`.
    presence: Vec<(FeedId, Vec<u8>)>,
    /// The link work the core has due (until it is reported, like the real core's) and the outcomes it was
    /// given.
    link_work: Vec<LinkWork>,
    link_done: Vec<(LinkId, LinkOutcome)>,
}

impl HostData {
    fn new(feeds: Vec<FeedWork>) -> Self {
        Self {
            configured: true,
            server_url: URL.to_owned(),
            pending_entries: 0,
            parked_items: 0,
            fork_detected: false,
            identity: Some((account(), device())),
            log: vec![b"genesis".to_vec()],
            published: 1,
            feed_lists: VecDeque::from([feeds]),
            outbox: HashMap::new(),
            membership: HashMap::new(),
            wanted: HashMap::new(),
            candidates: HashMap::new(),
            applied: HashMap::new(),
            cursors: HashMap::new(),
            pages_applied: Vec::new(),
            snapshots_applied: HashMap::new(),
            marks_wanted: HashMap::new(),
            marks_docs: HashMap::new(),
            chunk_work: HashMap::new(),
            needed_logs: VecDeque::new(),
            foreign_logs: Vec::new(),
            states: Vec::new(),
            recovery_blob: None,
            recovery_obsolete: Vec::new(),
            chunk_gone: Vec::new(),
            epoch_reset: false,
            pushed: Vec::new(),
            posted: Vec::new(),
            refused: Vec::new(),
            lost: Vec::new(),
            verified: Vec::new(),
            chunks_told: Vec::new(),
            fail: HashMap::new(),
            fail_feed: HashMap::new(),
            token: None,
            unknown_asked: 0,
            inline_outcomes: VecDeque::new(),
            inline_calls: Vec::new(),
            debounce_ms: PUSH_DEBOUNCE_MS,
            presence: Vec::new(),
            link_work: Vec::new(),
            link_done: Vec::new(),
        }
    }
}

struct FakeHost {
    data: Mutex<HostData>,
    log: Log,
    listeners: Arc<Mutex<Vec<(u64, Listener)>>>,
    next_listener: AtomicU64,
}

impl FakeHost {
    fn new(log: &Log, feeds: Vec<FeedWork>) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(HostData::new(feeds)),
            log: Arc::clone(log),
            listeners: Arc::default(),
            next_listener: AtomicU64::new(0),
        })
    }

    fn with<T>(&self, look: impl FnOnce(&mut HostData) -> T) -> T {
        look(&mut self.data.lock().unwrap())
    }

    fn call<T>(
        &self,
        name: &'static str,
        feed: Option<FeedId>,
        answer: impl FnOnce(&mut HostData) -> T,
    ) -> Result<T, ApiError> {
        self.log.lock().unwrap().push(match feed {
            Some(feed) => format!("h:{name}#{}", tag(feed)),
            None => format!("h:{name}"),
        });
        let mut data = self.data.lock().unwrap();
        if let Some(feed) = feed
            && let Some(error) = data
                .fail_feed
                .get_mut(&(name, feed))
                .and_then(VecDeque::pop_front)
        {
            return Err(error);
        }
        if let Some(error) = data.fail.get_mut(name).and_then(VecDeque::pop_front) {
            return Err(error);
        }
        Ok(answer(&mut data))
    }

    fn fail(&self, name: &'static str, error: ApiError) {
        self.with(|data| data.fail.entry(name).or_default().push_back(error));
    }

    /// The next call of `name` for `feed` fails.
    fn fail_feed(&self, name: &'static str, feed: FeedId, error: ApiError) {
        self.with(|data| {
            data.fail_feed
                .entry((name, feed))
                .or_default()
                .push_back(error);
        });
    }

    fn emit(&self, event: &Event) {
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(event);
        }
    }

    fn listener_count(&self) -> usize {
        self.listeners.lock().unwrap().len()
    }

    /// Cycles that ended: `Idle`, `Forked`, `Error` or `Offline` were reported that many times.
    fn cycles(&self) -> usize {
        self.with(|data| {
            data.states
                .iter()
                .filter(|(state, _)| {
                    matches!(
                        state,
                        SyncState::Idle | SyncState::Forked | SyncState::Error | SyncState::Offline
                    )
                })
                .count()
        })
    }

    fn last_state(&self) -> (SyncState, Option<String>) {
        self.with(|data| data.states.last().cloned().unwrap())
    }
}

impl SyncHost for FakeHost {
    fn sync_info(&self) -> Result<SyncInfo, ApiError> {
        self.call("sync_info", None, |data| SyncInfo {
            configured: data.configured,
            server_url: data.configured.then(|| data.server_url.clone()),
            state: SyncState::Idle,
            pending_entries: data.pending_entries,
            parked_items: data.parked_items,
            remote_cursor: 0,
            server_epoch: None,
            last_push_at: None,
            last_pull_at: None,
            last_error: None,
            fork_detected: data.fork_detected,
            devices_seen: 0,
            needs_invite_token: false,
            transfers: kb_types::TransferInfo::default(),
            feeds: Vec::new(),
        })
    }
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.call("sync_identity", None, |data| data.identity)
    }
    fn sync_take_invite_token(&self) -> Result<Option<String>, ApiError> {
        self.call("sync_take_invite_token", None, |data| data.token.take())
    }
    fn sync_account_unknown(&self) -> Result<(), ApiError> {
        // Like the core: the status asks for an invite token.
        self.call("sync_account_unknown", None, |data| {
            data.unknown_asked += 1;
            data.states.push((
                SyncState::Error,
                Some("the server does not know this account".to_owned()),
            ));
        })
    }
    fn sync_log_received(&self, _: &[Vec<u8>]) -> Result<bool, ApiError> {
        self.call("sync_log_received", None, |_| false)
    }
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.call("sync_account_log", None, |data| {
            (data.log.clone(), data.published)
        })
    }
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.call("sync_log_published", None, |data| {
            data.published = up_to_seq + 1;
        })
    }
    fn sync_sign(&self, message: &[u8]) -> Result<Signature, ApiError> {
        self.call("sync_sign", None, |_| {
            let key = kb_crypto::SigningKey::from_seed(&kb_crypto::Secret32::from_bytes([1; 32]));
            kb_crypto::sign(&key, "auth", message)
        })
    }
    fn sync_remote_epoch(&self, _: [u8; 16]) -> Result<bool, ApiError> {
        self.call("sync_remote_epoch", None, |data| data.epoch_reset)
    }
    fn sync_set_state(&self, state: SyncState, error: Option<String>) -> Result<(), ApiError> {
        self.call("sync_set_state", None, |data| {
            data.states.push((state, error))
        })
    }
    fn sync_recovery_blob(&self) -> Result<Option<RecoveryUpload>, ApiError> {
        self.call("sync_recovery_blob", None, |data| data.recovery_blob.take())
    }
    fn sync_recovery_published(&self) -> Result<(), ApiError> {
        self.call("sync_recovery_published", None, |_| ())
    }
    fn sync_recovery_obsolete(&self) -> Result<Vec<[u8; 32]>, ApiError> {
        self.call("sync_recovery_obsolete", None, |data| {
            data.recovery_obsolete.clone()
        })
    }
    fn sync_recovery_deleted(&self, id: [u8; 32]) -> Result<(), ApiError> {
        self.call("sync_recovery_deleted", None, |data| {
            data.recovery_obsolete.retain(|other| *other != id);
        })
    }
    fn sync_feeds(&self) -> Result<Vec<FeedWork>, ApiError> {
        self.call("sync_feeds", None, |data| {
            let list = if data.feed_lists.len() > 1 {
                data.feed_lists.pop_front().unwrap_or_default()
            } else {
                data.feed_lists.front().cloned().unwrap_or_default()
            };
            // The cursor the core holds for a feed it just learned of.
            list.into_iter()
                .map(|mut feed| {
                    let cursor = *data.cursors.entry(feed.feed).or_insert(feed.cursor);
                    feed.cursor = cursor;
                    feed
                })
                .collect()
        })
    }
    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError> {
        self.call("sync_needed_logs", None, |data| {
            data.needed_logs.pop_front().unwrap_or_default()
        })
    }
    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        _: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.call("sync_foreign_log_received", None, |data| {
            data.foreign_logs.push(account);
            true
        })
    }
    fn sync_membership_pending(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError> {
        self.call("sync_membership_pending", Some(feed), |data| {
            data.membership.get(&feed).cloned().unwrap_or_default()
        })
    }
    fn sync_membership_posted(&self, feed: FeedId, head: MembershipHead) -> Result<(), ApiError> {
        self.call("sync_membership_posted", Some(feed), |data| {
            data.membership.remove(&feed);
            data.posted.push((feed, head.seq));
        })
    }
    fn sync_membership_conflict(&self, feed: FeedId) -> Result<(), ApiError> {
        self.call("sync_membership_conflict", Some(feed), |_| ())
    }
    fn sync_membership_wanted(&self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError> {
        self.call("sync_membership_wanted", Some(feed), |data| {
            data.wanted.get_mut(&feed).and_then(VecDeque::pop_front)
        })
    }
    fn sync_membership_fetched(&self, feed: FeedId, _: &MembershipPage) -> Result<(), ApiError> {
        self.call("sync_membership_fetched", Some(feed), |_| ())
    }
    fn sync_feed_lost(&self, feed: FeedId) -> Result<(), ApiError> {
        self.call("sync_feed_lost", Some(feed), |data| data.lost.push(feed))
    }
    fn sync_feed_refused(&self, feed: FeedId, _: &str) -> Result<(), ApiError> {
        self.call("sync_feed_refused", Some(feed), |data| {
            data.refused.push(feed)
        })
    }
    fn sync_feed_rekey_needed(&self, feed: FeedId) -> Result<(), ApiError> {
        self.call("sync_feed_rekey_needed", Some(feed), |_| ())
    }
    fn sync_feed_outbox(&self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError> {
        self.call("sync_feed_outbox", Some(feed), |data| {
            data.outbox.get(&feed).cloned().unwrap_or_default()
        })
    }
    fn sync_feed_pushed(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        _: i64,
    ) -> Result<(), ApiError> {
        self.call("sync_feed_pushed", Some(feed), |data| {
            if let Some(outbox) = data.outbox.get_mut(&feed) {
                outbox.retain(|entry| !(entry.doc_id == doc_id && entry.device_seq == device_seq));
            }
            data.pushed.push((feed, doc_id, device_seq, cursor));
        })
    }
    fn sync_feed_apply(&self, feed: FeedId, page: &ChangesPage) -> Result<SyncApplied, ApiError> {
        self.call("sync_feed_apply", Some(feed), |data| {
            let cursors: Vec<u64> = page.changes.iter().map(|(cursor, _)| *cursor).collect();
            data.pages_applied.push((feed, cursors));
            let held = data.cursors.get(&feed).copied().unwrap_or(0);
            let scripted = data.applied.get_mut(&feed).and_then(VecDeque::pop_front);
            let report = scripted.unwrap_or_else(|| SyncApplied {
                applied: u32::try_from(page.changes.len()).unwrap_or(u32::MAX),
                new_cursor: page.changes.last().map_or(held, |(cursor, _)| *cursor),
                ..SyncApplied::default()
            });
            data.cursors.insert(feed, report.new_cursor.max(held));
            report
        })
    }
    fn sync_feed_snapshot_candidates(&self, feed: FeedId) -> Result<Vec<DocId>, ApiError> {
        self.call("sync_feed_snapshot_candidates", Some(feed), |data| {
            data.candidates.get(&feed).cloned().unwrap_or_default()
        })
    }
    fn sync_feed_make_snapshot(&self, feed: FeedId, _: DocId) -> Result<(Vec<u8>, u64), ApiError> {
        self.call("sync_feed_make_snapshot", Some(feed), |_| {
            (b"snapshot".to_vec(), 7)
        })
    }
    fn sync_feed_snapshot_published(
        &self,
        feed: FeedId,
        doc_id: DocId,
        _: u64,
    ) -> Result<(), ApiError> {
        self.call("sync_feed_snapshot_published", Some(feed), |data| {
            if let Some(list) = data.candidates.get_mut(&feed) {
                list.retain(|candidate| *candidate != doc_id);
            }
        })
    }
    fn sync_feed_apply_snapshots(
        &self,
        feed: FeedId,
        _: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError> {
        self.call("sync_feed_apply_snapshots", Some(feed), |data| {
            data.snapshots_applied
                .get_mut(&feed)
                .and_then(VecDeque::pop_front)
                .unwrap_or_default()
        })
    }
    fn sync_snapshot_marks_wanted(&self, feed: FeedId) -> Result<Option<u64>, ApiError> {
        self.call("sync_snapshot_marks_wanted", Some(feed), |data| {
            data.marks_wanted
                .get_mut(&feed)
                .and_then(VecDeque::pop_front)
                .flatten()
        })
    }
    fn sync_snapshot_marks_received(
        &self,
        feed: FeedId,
        _: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError> {
        self.call("sync_snapshot_marks_received", Some(feed), |data| {
            data.marks_docs.remove(&feed).unwrap_or_default()
        })
    }
    fn sync_snapshot_verified(
        &self,
        feed: FeedId,
        doc_id: DocId,
        ok: bool,
    ) -> Result<(), ApiError> {
        self.call("sync_snapshot_verified", Some(feed), |data| {
            data.verified.push((feed, doc_id, ok));
        })
    }
    fn sync_feed_head(&self, feed: FeedId) -> Result<Vec<u8>, ApiError> {
        self.call("sync_feed_head", Some(feed), |_| b"head".to_vec())
    }
    fn sync_feed_heads_received(&self, feed: FeedId, _: &[Vec<u8>]) -> Result<bool, ApiError> {
        self.call("sync_feed_heads_received", Some(feed), |_| false)
    }
    fn sync_feed_chunk_work(
        &self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError> {
        self.call("sync_feed_chunk_work", Some(feed), |data| {
            data.chunk_work
                .get(&feed)
                .map(|items| items.iter().take(limit).copied().collect())
                .unwrap_or_default()
        })
    }
    fn sync_feed_chunk_bytes(
        &self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        self.call("sync_feed_chunk_bytes", Some(feed), |data| {
            (!data.chunk_gone.contains(&hash)).then(|| b"sealed".to_vec())
        })
    }
    fn sync_feed_chunk_uploaded(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.call("sync_feed_chunk_uploaded", Some(feed), |data| {
            data.chunks_told.push(("uploaded", feed, hash));
            done_chunk(data, feed, hash);
        })
    }
    fn sync_feed_chunk_received(
        &self,
        feed: FeedId,
        hash: ChunkId,
        _: Vec<u8>,
    ) -> Result<(), ApiError> {
        self.call("sync_feed_chunk_received", Some(feed), |data| {
            data.chunks_told.push(("received", feed, hash));
            done_chunk(data, feed, hash);
        })
    }
    fn sync_feed_chunk_unavailable(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.call("sync_feed_chunk_unavailable", Some(feed), |data| {
            data.chunks_told.push(("unavailable", feed, hash));
            done_chunk(data, feed, hash);
        })
    }
    fn sync_feed_chunk_deleted(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.call("sync_feed_chunk_deleted", Some(feed), |data| {
            data.chunks_told.push(("deleted", feed, hash));
            done_chunk(data, feed, hash);
        })
    }
    fn sync_feed_apply_inline(
        &self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError> {
        self.call("sync_feed_apply_inline", Some(feed), |data| {
            data.inline_calls.push((feed, inline.cursor));
            let held = data.cursors.get(&feed).copied().unwrap_or(0);
            let outcome = data
                .inline_outcomes
                .pop_front()
                .unwrap_or(InlineOutcome::Pull);
            if let InlineOutcome::Applied(applied) = outcome {
                data.cursors.insert(feed, applied.new_cursor.max(held));
            }
            outcome
        })
    }
    fn sync_push_debounce_ms(&self) -> Result<u64, ApiError> {
        self.call("sync_push_debounce_ms", None, |data| data.debounce_ms)
    }
    fn presence_received(&self, feed: FeedId, envelope: &[u8]) -> Result<(), ApiError> {
        self.call("presence_received", Some(feed), |data| {
            data.presence.push((feed, envelope.to_vec()));
        })
    }
    fn presence_expire(&self) -> Result<(), ApiError> {
        self.call("presence_expire", None, |_| ())
    }
    fn sync_link_work(&self) -> Result<Vec<LinkWork>, ApiError> {
        self.call("sync_link_work", None, |data| data.link_work.clone())
    }
    fn sync_link_done(&self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError> {
        self.call("sync_link_done", None, |data| {
            data.link_work.retain(|work| work.link_id() != link_id);
            data.link_done.push((link_id, outcome));
        })
    }
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        let id = self.next_listener.fetch_add(1, Ordering::Relaxed);
        self.listeners
            .lock()
            .unwrap()
            .push((id, Arc::from(on_event)));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || listeners.lock().unwrap().retain(|(other, _)| *other != id))
    }
}

/// The core recorded a chunk's outcome: it is not due any more.
fn done_chunk(data: &mut HostData, feed: FeedId, hash: ChunkId) {
    if let Some(items) = data.chunk_work.get_mut(&feed) {
        items.retain(|item| item.hash != hash);
    }
}

// ----- the fake remote -------------------------------------------------------------------------

struct RemoteData {
    protocol: u8,
    /// The account's log, `None` while the server does not know the account.
    log: Option<Vec<Vec<u8>>>,
    changes: HashMap<FeedId, VecDeque<ChangesPage>>,
    marks: HashMap<FeedId, VecDeque<MarksPage>>,
    membership: HashMap<FeedId, VecDeque<MembershipPage>>,
    snapshots: HashMap<(FeedId, DocId), Vec<u8>>,
    chunks: HashMap<(FeedId, ChunkId), Vec<u8>>,
    fail: HashMap<&'static str, VecDeque<SyncError>>,
    /// Failures of one feed's call.
    fail_feed: HashMap<(&'static str, FeedId), VecDeque<SyncError>>,
    next_cursor: u64,
    authenticated: bool,
    appended: Vec<(FeedId, Vec<u8>)>,
    put_chunks: Vec<(FeedId, ChunkId)>,
    disputes: Vec<(FeedId, DocId, u64)>,
    heads_put: Vec<FeedId>,
    created: Vec<FeedId>,
    authentications: usize,
    /// The realtime sockets `realtime` hands out, one per call (none: the remote has no socket).
    sockets: VecDeque<Realtime>,
    /// When each call arrived (the paused clock).
    times: Vec<(&'static str, Instant)>,
    /// A scripted `Unauthorized` leaves the session in place (a 403 would).
    keep_session: bool,
    /// `authenticate` asks the signer to sign, as the real remotes do.
    sign: bool,
    recovery: Vec<([u8; 32], Vec<u8>)>,
    recovery_deletes: Vec<[u8; 32]>,
    snapshot_puts: usize,
    /// `(calls that pass first, the error of the next one)`.
    fail_nth: HashMap<&'static str, (usize, SyncError)>,
}

struct FakeRemote {
    data: Mutex<RemoteData>,
    log: Log,
}

impl FakeRemote {
    fn new(log: &Log) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(RemoteData {
                protocol: PROTOCOL_VERSION,
                log: Some(vec![b"genesis".to_vec()]),
                changes: HashMap::new(),
                marks: HashMap::new(),
                membership: HashMap::new(),
                snapshots: HashMap::new(),
                chunks: HashMap::new(),
                fail: HashMap::new(),
                fail_feed: HashMap::new(),
                next_cursor: 100,
                authenticated: false,
                appended: Vec::new(),
                put_chunks: Vec::new(),
                disputes: Vec::new(),
                heads_put: Vec::new(),
                created: Vec::new(),
                authentications: 0,
                sockets: VecDeque::new(),
                times: Vec::new(),
                keep_session: false,
                sign: false,
                recovery: Vec::new(),
                recovery_deletes: Vec::new(),
                snapshot_puts: 0,
                fail_nth: HashMap::new(),
            }),
            log: Arc::clone(log),
        })
    }

    fn with<T>(&self, look: impl FnOnce(&mut RemoteData) -> T) -> T {
        look(&mut self.data.lock().unwrap())
    }

    /// When the calls named `name` arrived.
    fn times(&self, name: &str) -> Vec<Instant> {
        self.with(|data| {
            data.times
                .iter()
                .filter(|(call, _)| *call == name)
                .map(|(_, at)| *at)
                .collect()
        })
    }

    /// Fails every call of `name` with `error` until [`FakeRemote::heal`].
    fn fail_always(&self, name: &'static str, error: &SyncError) {
        self.with(|data| {
            data.fail
                .insert(name, std::iter::repeat_n(error.clone(), 64).collect());
        });
    }

    /// The call of `name` after `passes` ones that succeed fails with `error`.
    fn fail_nth(&self, name: &'static str, passes: usize, error: SyncError) {
        self.with(|data| data.fail_nth.insert(name, (passes, error)));
    }

    fn heal(&self, name: &'static str) {
        self.with(|data| data.fail.remove(name));
    }

    fn fail_feed(&self, name: &'static str, feed: FeedId, error: SyncError) {
        self.with(|data| {
            data.fail_feed
                .entry((name, feed))
                .or_default()
                .push_back(error);
        });
    }

    fn fail(&self, name: &'static str, error: SyncError) {
        self.with(|data| data.fail.entry(name).or_default().push_back(error));
    }

    fn answer<'a, T: Send + 'a>(
        &self,
        name: &'static str,
        feed: Option<FeedId>,
        ok: impl FnOnce(&mut RemoteData) -> T,
    ) -> RemoteFuture<'a, T> {
        self.log.lock().unwrap().push(match feed {
            Some(feed) => format!("r:{name}#{}", tag(feed)),
            None => format!("r:{name}"),
        });
        let mut data = self.data.lock().unwrap();
        data.times.push((name, Instant::now()));
        let nth = match data.fail_nth.get_mut(name) {
            Some((passes, _)) if *passes > 0 => {
                *passes -= 1;
                None
            }
            Some(_) => data.fail_nth.remove(name).map(|(_, error)| error),
            None => None,
        };
        let scripted = nth
            .or_else(|| {
                feed.and_then(|feed| {
                    data.fail_feed
                        .get_mut(&(name, feed))
                        .and_then(VecDeque::pop_front)
                })
            })
            .or_else(|| data.fail.get_mut(name).and_then(VecDeque::pop_front));
        let result = match scripted {
            Some(error) => {
                // A 401 drops the session, as the HTTP remote does.
                if matches!(error, SyncError::Unauthorized) && !data.keep_session {
                    data.authenticated = false;
                }
                Err(error)
            }
            None => Ok(ok(&mut data)),
        };
        Box::pin(std::future::ready(result))
    }
}

impl SyncRemote for FakeRemote {
    fn server_info(&self) -> RemoteFuture<'_, ServerInfo> {
        self.answer("server_info", None, |data| ServerInfo {
            protocol_version: data.protocol,
            server_epoch: EPOCH,
            max_wire_bytes: 1 << 20,
            account_quota_bytes: None,
            idle_device_days: None,
            share_signups: 20,
            share_signups_per_account: 5,
        })
    }
    fn create_account<'a>(&'a self, _: &'a str, log: &'a [Vec<u8>]) -> RemoteFuture<'a, ()> {
        let log = log.to_vec();
        self.answer("create_account", None, move |data| data.log = Some(log))
    }
    fn account_create_share<'a>(
        &'a self,
        _: &'a [Vec<u8>],
        _: InviteId,
        _: &'a Signature,
    ) -> RemoteFuture<'a, ()> {
        self.answer("account_create_share", None, |_| ())
    }
    fn append_log<'a>(&'a self, _: AccountId, entries: &'a [Vec<u8>]) -> RemoteFuture<'a, ()> {
        let entries = entries.to_vec();
        self.answer("append_log", None, move |data| {
            data.log.get_or_insert_with(Vec::new).extend(entries);
        })
    }
    fn fetch_log(&self, _: AccountId, _: Option<u64>) -> RemoteFuture<'_, Vec<Vec<u8>>> {
        self.log.lock().unwrap().push("r:fetch_log".to_owned());
        let mut data = self.data.lock().unwrap();
        data.times.push(("fetch_log", Instant::now()));
        let result = match data.fail.get_mut("fetch_log").and_then(VecDeque::pop_front) {
            Some(error) => Err(error),
            None => data.log.clone().ok_or(SyncError::NotFound),
        };
        Box::pin(std::future::ready(result))
    }
    fn logs_batch<'a>(&'a self, requests: &'a [LogRequest]) -> RemoteFuture<'a, Vec<LogChunk>> {
        let requests = requests.to_vec();
        self.answer("logs_batch", None, move |_| {
            requests
                .iter()
                .map(|request| LogChunk {
                    account: request.account,
                    entries: vec![b"log".to_vec()],
                })
                .collect()
        })
    }
    fn needs_authentication(&self, _: AccountId, _: DeviceId) -> bool {
        !self.with(|data| data.authenticated)
    }
    fn authenticate(&self, _: AccountId, _: DeviceId, signer: AuthSigner) -> RemoteFuture<'_, ()> {
        let signed = self.with(|data| data.sign).then(|| signer(b"challenge"));
        if signed == Some(None) {
            self.log.lock().unwrap().push("r:authenticate".to_owned());
            return Box::pin(std::future::ready(Err(SyncError::Unauthorized)));
        }
        self.answer("authenticate", None, |data| {
            data.authenticated = true;
            data.authentications += 1;
        })
    }
    fn feeds_create<'a>(&'a self, create: &'a [u8]) -> RemoteFuture<'a, FeedCreated> {
        let feed = kb_sync::v2::peek_membership(create).unwrap().feed;
        self.answer("feeds_create", Some(feed), move |data| {
            data.created.push(feed);
            FeedCreated {
                feed,
                head: MembershipHead {
                    seq: 0,
                    hash: [1; 32],
                },
                cursor: 1,
            }
        })
    }
    fn feeds_list(&self) -> RemoteFuture<'_, Vec<FeedListing>> {
        self.answer("feeds_list", None, |_| Vec::new())
    }
    fn membership_append<'a>(
        &'a self,
        feed: FeedId,
        entries: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, MembershipHead> {
        let seq = entries
            .last()
            .map_or(0, |entry| kb_sync::v2::peek_membership(entry).unwrap().seq);
        self.answer("membership_append", Some(feed), move |_| MembershipHead {
            seq,
            hash: [2; 32],
        })
    }
    fn membership_get(&self, feed: FeedId, _: Option<u64>) -> RemoteFuture<'_, MembershipPage> {
        self.answer("membership_get", Some(feed), move |data| {
            data.membership
                .get_mut(&feed)
                .and_then(VecDeque::pop_front)
                .unwrap_or(MembershipPage {
                    entries: Vec::new(),
                    head: MembershipHead {
                        seq: 0,
                        hash: [0; 32],
                    },
                    more: false,
                })
        })
    }
    fn append<'a>(&'a self, feed: FeedId, entry: &'a [u8]) -> RemoteFuture<'a, (u64, [u8; 32])> {
        let entry = entry.to_vec();
        self.answer("append", Some(feed), move |data| {
            data.appended.push((feed, entry));
            data.next_cursor += 1;
            (data.next_cursor, [0; 32])
        })
    }
    fn changes(&self, feed: FeedId, _: u64, _: u32) -> RemoteFuture<'_, ChangesPage> {
        self.answer("changes", Some(feed), move |data| {
            data.changes
                .get_mut(&feed)
                .and_then(VecDeque::pop_front)
                .unwrap_or_else(|| page(&[], false))
        })
    }
    fn put_snapshot<'a>(&'a self, feed: FeedId, _: &'a [u8]) -> RemoteFuture<'a, ()> {
        self.answer("put_snapshot", Some(feed), |data| data.snapshot_puts += 1)
    }
    fn get_snapshot(&self, feed: FeedId, doc_id: DocId) -> RemoteFuture<'_, Option<Vec<u8>>> {
        self.answer("get_snapshot", Some(feed), move |data| {
            data.snapshots.get(&(feed, doc_id)).cloned()
        })
    }
    fn snapshots(&self, feed: FeedId) -> RemoteFuture<'_, SnapshotsPage> {
        self.answer("snapshots", Some(feed), |_| SnapshotsPage {
            server_epoch: EPOCH,
            snapshots: Vec::new(),
        })
    }
    fn snapshot_marks(&self, feed: FeedId, _: u64) -> RemoteFuture<'_, MarksPage> {
        self.answer("snapshot_marks", Some(feed), move |data| {
            data.marks
                .get_mut(&feed)
                .and_then(VecDeque::pop_front)
                .unwrap_or(MarksPage {
                    marks: Vec::new(),
                    more: false,
                })
        })
    }
    fn snapshot_dispute(&self, feed: FeedId, doc_id: DocId, covers: u64) -> RemoteFuture<'_, ()> {
        self.answer("snapshot_dispute", Some(feed), move |data| {
            data.disputes.push((feed, doc_id, covers));
        })
    }
    fn put_head<'a>(&'a self, feed: FeedId, _: &'a [u8]) -> RemoteFuture<'a, ()> {
        self.answer("put_head", Some(feed), move |data| {
            data.heads_put.push(feed)
        })
    }
    fn heads(&self, feed: FeedId) -> RemoteFuture<'_, Vec<Vec<u8>>> {
        self.answer("heads", Some(feed), |_| Vec::new())
    }
    fn put_chunk<'a>(&'a self, feed: FeedId, hash: ChunkId, _: &'a [u8]) -> RemoteFuture<'a, ()> {
        self.answer("put_chunk", Some(feed), move |data| {
            data.put_chunks.push((feed, hash));
        })
    }
    fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, Option<Vec<u8>>> {
        self.answer("get_chunk", Some(feed), move |data| {
            data.chunks.get(&(feed, hash)).cloned()
        })
    }
    fn delete_chunk(&self, feed: FeedId, _: ChunkId) -> RemoteFuture<'_, ()> {
        self.answer("delete_chunk", Some(feed), |_| ())
    }
    fn copy_chunk(&self, feed: FeedId, _: ChunkId) -> RemoteFuture<'_, ()> {
        self.answer("copy_chunk", Some(feed), |_| ())
    }
    fn invite_open<'a>(&'a self, _: InviteId, _: &'a Signature) -> RemoteFuture<'a, InviteOpened> {
        self.answer("invite_open", None, |_| InviteOpened {
            feed: shared_a(),
            log: Vec::new(),
        })
    }
    fn link_put<'a>(&'a self, _: LinkId, _: &'a LinkPut) -> RemoteFuture<'a, u64> {
        self.answer("link_put", None, |_| 7)
    }
    fn link_delete(&self, _: LinkId) -> RemoteFuture<'_, ()> {
        self.answer("link_delete", None, |_| ())
    }
    fn realtime(&self) -> Option<Realtime> {
        self.log.lock().unwrap().push("r:realtime".to_owned());
        self.with(|data| data.sockets.pop_front())
    }
    fn pair_create<'a>(&'a self, _: &'a [u8]) -> RemoteFuture<'a, ([u8; 6], i64)> {
        self.answer("pair_create", None, |_| ([0; 6], 0))
    }
    fn pair_messages(&self, _: [u8; 6], _: u32, _: u32) -> RemoteFuture<'_, PairingPage> {
        self.answer("pair_messages", None, |_| PairingPage {
            messages: Vec::new(),
            expires_at: 0,
            closed: false,
        })
    }
    fn pair_post<'a>(&'a self, _: [u8; 6], _: u8, _: &'a [u8]) -> RemoteFuture<'a, ()> {
        self.answer("pair_post", None, |_| ())
    }
    fn pair_close(&self, _: [u8; 6]) -> RemoteFuture<'_, ()> {
        self.answer("pair_close", None, |_| ())
    }
    fn recovery_put<'a>(&'a self, id: [u8; 32], bytes: &'a [u8]) -> RemoteFuture<'a, ()> {
        let bytes = bytes.to_vec();
        self.answer("recovery_put", None, move |data| {
            data.recovery.retain(|(other, _)| *other != id);
            data.recovery.push((id, bytes));
        })
    }
    fn recovery_get(&self, id: [u8; 32]) -> RemoteFuture<'_, Option<Vec<u8>>> {
        self.answer("recovery_get", None, move |data| {
            data.recovery
                .iter()
                .find(|(other, _)| *other == id)
                .map(|(_, bytes)| bytes.clone())
        })
    }
    fn recovery_delete(&self, id: [u8; 32]) -> RemoteFuture<'_, ()> {
        self.answer("recovery_delete", None, move |data| {
            data.recovery_deletes.push(id);
            data.recovery.retain(|(other, _)| *other != id);
        })
    }
}

// ----- the rig ---------------------------------------------------------------------------------

struct Rig {
    host: Arc<FakeHost>,
    remote: Arc<FakeRemote>,
    log: Log,
    /// The server addresses the factory was asked for.
    urls: Arc<Mutex<Vec<String>>>,
    _handle: Option<DriverHandle>,
}

impl Rig {
    /// A host with `feeds`, a remote, and no driver yet.
    fn new(feeds: Vec<FeedWork>) -> Self {
        let log: Log = Arc::default();
        Self {
            host: FakeHost::new(&log, feeds),
            remote: FakeRemote::new(&log),
            log,
            urls: Arc::default(),
            _handle: None,
        }
    }

    /// Starts the driver: its first cycle runs at once.
    fn start(&mut self) {
        let remote = Arc::clone(&self.remote);
        let urls = Arc::clone(&self.urls);
        let factory: RemoteFactory<FakeRemote> = Arc::new(move |url: &str| {
            urls.lock().unwrap().push(url.to_owned());
            Ok(Arc::clone(&remote))
        });
        let host: Arc<dyn SyncHost> = Arc::clone(&self.host) as Arc<dyn SyncHost>;
        self._handle = Some(SyncDriver::spawn(
            host,
            factory,
            tokio::runtime::Handle::current(),
        ));
    }

    /// Lets the paused clock run until `cycles` cycles ended.
    async fn until_cycles(&self, cycles: usize) {
        for _ in 0..200_000 {
            if self.host.cycles() >= cycles {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!(
            "never ended {cycles} cycles: {:?}",
            self.log.lock().unwrap()
        );
    }

    /// Runs one cycle to its end.
    async fn one_cycle(&mut self) {
        self.start();
        self.until_cycles(1).await;
    }

    /// The calls of the log that name one of `names` (host `h:` or remote `r:`), in order.
    fn calls(&self, names: &[&str]) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|call| {
                let name = call.split_once(':').map_or(call.as_str(), |(_, rest)| rest);
                let name = name.split('#').next().unwrap_or(name);
                names.contains(&name)
            })
            .cloned()
            .collect()
    }

    /// The whole log, with a plain-text marker for each cycle step it names.
    fn count(&self, call: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|logged| logged.as_str() == call)
            .count()
    }
}

// ----- the order of a cycle --------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_cycle_runs_the_personal_feed_then_the_logs_then_each_shared_feed() {
    let mut rig = Rig::new(vec![
        personal_work(),
        shared_work(shared_a()),
        shared_work(shared_b()),
    ]);
    rig.host.with(|data| {
        data.needed_logs = VecDeque::from([vec![LogRequest {
            account: AccountId::from_bytes([9; 16]),
            after: None,
        }]]);
        for feed in [personal(), shared_a(), shared_b()] {
            data.outbox.insert(feed, vec![outbound(1, 1)]);
            data.candidates.insert(feed, vec![doc(1)]);
        }
        data.marks_wanted
            .insert(shared_a(), VecDeque::from([Some(0)]));
        data.wanted.insert(shared_a(), VecDeque::from([None]));
    });
    rig.remote.with(|data| {
        data.changes
            .insert(shared_a(), VecDeque::from([page(&[5], false)]));
    });
    rig.one_cycle().await;
    assert_eq!(rig.host.last_state().0, SyncState::Idle);

    let steps = rig.calls(&[
        "server_info",
        "sync_remote_epoch",
        "fetch_log",
        "authenticate",
        "sync_feeds",
        "sync_needed_logs",
        "logs_batch",
        "sync_foreign_log_received",
        "sync_membership_pending",
        "sync_membership_wanted",
        "membership_get",
        "sync_membership_fetched",
        "sync_feed_outbox",
        "append",
        "sync_feed_pushed",
        "sync_feed_snapshot_candidates",
        "sync_feed_make_snapshot",
        "put_snapshot",
        "sync_feed_snapshot_published",
        "changes",
        "sync_feed_apply",
        "sync_snapshot_marks_wanted",
        "snapshot_marks",
        "sync_feed_chunk_work",
        "sync_feed_head",
        "put_head",
        "heads",
        "sync_feed_heads_received",
    ]);
    let feed_steps = |tag: u8| -> Vec<String> {
        [
            "sync_feed_outbox",
            "append",
            "sync_feed_pushed",
            "sync_feed_snapshot_candidates",
            "sync_feed_make_snapshot",
            "put_snapshot",
            "sync_feed_snapshot_published",
            "changes",
            "sync_feed_apply",
        ]
        .iter()
        .map(|name| {
            // `h:` or `r:`, by what the log says.
            let host = name.starts_with("sync_");
            format!("{}:{name}#{tag}", if host { "h" } else { "r" })
        })
        .collect()
    };
    let mut expected: Vec<String> = [
        "r:server_info",
        "h:sync_remote_epoch",
        "r:fetch_log",
        "r:authenticate",
        "h:sync_feeds",
    ]
    .map(str::to_owned)
    .to_vec();
    // The personal feed: push, snapshots, pull, head.
    expected.extend(feed_steps(1));
    expected.extend(
        [
            "h:sync_feed_head#1",
            "r:put_head#1",
            "r:heads#1",
            "h:sync_feed_heads_received#1",
        ]
        .map(str::to_owned),
    );
    // The co-members' logs, then the feeds listed again.
    expected.extend(
        [
            "h:sync_needed_logs",
            "r:logs_batch",
            "h:sync_foreign_log_received",
            "h:sync_feeds",
        ]
        .map(str::to_owned),
    );
    // Shared feed A: membership read, push, snapshots, pull, marks, head.
    expected.extend(
        [
            "h:sync_membership_pending#2",
            "h:sync_membership_wanted#2",
            "r:membership_get#2",
            "h:sync_membership_fetched#2",
        ]
        .map(str::to_owned),
    );
    expected.extend(feed_steps(2));
    expected.extend(
        [
            "h:sync_snapshot_marks_wanted#2",
            "r:snapshot_marks#2",
            "h:sync_feed_head#2",
            "r:put_head#2",
            "r:heads#2",
            "h:sync_feed_heads_received#2",
        ]
        .map(str::to_owned),
    );
    // Shared feed B.
    expected
        .extend(["h:sync_membership_pending#3", "h:sync_membership_wanted#3"].map(str::to_owned));
    expected.extend(feed_steps(3));
    expected.extend(
        [
            "h:sync_snapshot_marks_wanted#3",
            "h:sync_feed_head#3",
            "r:put_head#3",
            "r:heads#3",
            "h:sync_feed_heads_received#3",
        ]
        .map(str::to_owned),
    );
    // The chunk step asks the core for work of every feed (nothing is due).
    let steps: Vec<String> = steps
        .into_iter()
        .filter(|call| !call.contains("sync_feed_chunk_work"))
        .collect();
    assert_eq!(steps, expected);
    assert_eq!(rig.count("h:sync_feed_chunk_work#1"), 1);
    assert_eq!(rig.count("h:sync_feed_chunk_work#2"), 1);
    assert_eq!(rig.count("h:sync_feed_chunk_work#3"), 1);
    // The co-member's log reached the core.
    assert_eq!(
        rig.host.with(|data| data.foreign_logs.clone()),
        vec![AccountId::from_bytes([9; 16])]
    );
}

#[tokio::test(start_paused = true)]
async fn a_shared_feed_the_personal_pull_taught_is_worked_in_the_same_cycle() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        // The second listing (after the personal feed) knows a space the account document just taught.
        data.feed_lists
            .push_back(vec![personal_work(), shared_work(shared_a())]);
    });
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_feeds"), 2);
    assert_eq!(
        rig.count("h:sync_feed_head#2"),
        1,
        "the new feed was worked"
    );
    assert_eq!(rig.count("h:sync_feed_head#1"), 1);
}

#[tokio::test(start_paused = true)]
async fn the_create_goes_through_feeds_create_and_the_rest_is_appended() {
    let (create, profile) = membership_entries();
    let feed = kb_sync::v2::peek_membership(&create).unwrap().feed;
    let mut rig = Rig::new(vec![personal_work(), shared_work(feed)]);
    rig.host
        .with(|data| data.membership.insert(feed, vec![create, profile]));
    rig.one_cycle().await;
    let steps = rig.calls(&[
        "feeds_create",
        "membership_append",
        "sync_membership_posted",
    ]);
    let tag = tag(feed);
    assert_eq!(
        steps,
        vec![
            format!("r:feeds_create#{tag}"),
            format!("r:membership_append#{tag}"),
            format!("h:sync_membership_posted#{tag}"),
        ]
    );
    assert_eq!(
        rig.host.with(|data| data.posted.clone()),
        vec![(feed, 1)],
        "the head the server answered last"
    );
}

#[tokio::test(start_paused = true)]
async fn entries_after_the_create_are_appended_without_a_create() {
    let (_, profile) = membership_entries();
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host
        .with(|data| data.membership.insert(shared_a(), vec![profile]));
    rig.one_cycle().await;
    assert_eq!(rig.count("r:feeds_create#2"), 0);
    assert_eq!(rig.count("r:membership_append#2"), 1);
    assert_eq!(rig.count("h:sync_membership_posted#2"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_lost_compare_and_swap_is_reported_and_nothing_is_posted() {
    let (_, profile) = membership_entries();
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host
        .with(|data| data.membership.insert(shared_a(), vec![profile]));
    rig.remote
        .fail("membership_append", SyncError::Conflict("lost".to_owned()));
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_membership_conflict#2"), 1);
    assert_eq!(rig.count("h:sync_membership_posted#2"), 0);
    // The feed's other steps still ran.
    assert_eq!(rig.count("h:sync_feed_head#2"), 1);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_conflict_on_the_create_skips_the_rest_and_is_reported() {
    let (create, profile) = membership_entries();
    let feed = kb_sync::v2::peek_membership(&create).unwrap().feed;
    let mut rig = Rig::new(vec![personal_work(), shared_work(feed)]);
    rig.host
        .with(|data| data.membership.insert(feed, vec![create, profile]));
    rig.remote
        .fail("feeds_create", SyncError::Conflict("taken".to_owned()));
    rig.one_cycle().await;
    assert_eq!(rig.calls(&["membership_append"]), Vec::<String>::new());
    assert_eq!(
        rig.count(&format!("h:sync_membership_conflict#{}", tag(feed))),
        1
    );
}

// ----- a feed the server does not know ---------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_404_on_the_membership_append_loses_the_feed_and_ends_its_step() {
    let (_, profile) = membership_entries();
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host
        .with(|data| data.membership.insert(shared_a(), vec![profile]));
    rig.remote.fail("membership_append", SyncError::NotFound);
    rig.one_cycle().await;
    assert_eq!(rig.host.with(|data| data.lost.clone()), vec![shared_a()]);
    for later in ["h:sync_feed_outbox#2", "r:changes#2", "h:sync_feed_head#2"] {
        assert_eq!(rig.count(later), 0, "{later}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_404_on_a_push_or_on_a_pull_loses_the_feed() {
    for failing in ["append", "changes"] {
        let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
        rig.host
            .with(|data| data.outbox.insert(shared_a(), vec![outbound(1, 1)]));
        rig.remote
            .fail_feed(failing, shared_a(), SyncError::NotFound);
        rig.one_cycle().await;
        assert_eq!(
            rig.host.with(|data| data.lost.clone()),
            vec![shared_a()],
            "{failing}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_403_on_a_push_or_a_pull_is_reported_and_the_pull_still_runs() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.outbox
            .insert(shared_a(), vec![outbound(1, 1), outbound(2, 1)]);
    });
    rig.remote
        .fail_feed("append", shared_a(), SyncError::Forbidden);
    rig.one_cycle().await;
    assert_eq!(rig.host.with(|data| data.refused.clone()), vec![shared_a()]);
    assert_eq!(rig.count("r:append#2"), 1, "the push ended at the refusal");
    assert_eq!(rig.count("r:changes#2"), 1, "the pull still ran");
    assert_eq!(rig.count("h:sync_feed_head#2"), 1);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);

    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.remote
        .fail_feed("changes", shared_a(), SyncError::Forbidden);
    rig.one_cycle().await;
    assert_eq!(rig.host.with(|data| data.refused.clone()), vec![shared_a()]);
}

// ----- the refusals of a push ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn rekey_needed_is_told_to_the_core_and_ends_the_uploads_of_the_feed() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.outbox.insert(
            shared_a(),
            vec![outbound(1, 1), outbound(2, 1), outbound(3, 1)],
        );
    });
    rig.remote.fail("append", SyncError::RekeyNeeded);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:append#2"), 1);
    assert_eq!(rig.count("h:sync_feed_rekey_needed#2"), 1);
    assert_eq!(rig.count("h:sync_feed_pushed#2"), 0);
    assert_eq!(rig.count("r:changes#2"), 1, "the pull still ran");
}

#[tokio::test(start_paused = true)]
async fn a_stale_epoch_ends_the_push_and_the_outbox_is_tried_again_after_the_pull() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.outbox.insert(shared_a(), vec![outbound(1, 1)]);
    });
    rig.remote.fail("append", SyncError::StaleEpoch);
    rig.one_cycle().await;
    let order = rig.calls(&["sync_feed_outbox", "append", "changes", "sync_feed_pushed"]);
    let for_a: Vec<&str> = order
        .iter()
        .map(String::as_str)
        .filter(|call| call.ends_with("#2"))
        .collect();
    assert_eq!(
        for_a,
        vec![
            "h:sync_feed_outbox#2",
            "r:append#2",
            "r:changes#2",
            "h:sync_feed_outbox#2",
            "r:append#2",
            "h:sync_feed_pushed#2",
        ],
        "the second try follows the pull"
    );
    assert_eq!(rig.count("h:sync_feed_rekey_needed#2"), 0);
}

#[tokio::test(start_paused = true)]
async fn a_closed_feed_refuses_writes_quietly_and_a_flagged_one_is_never_written() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.outbox
            .insert(shared_a(), vec![outbound(1, 1), outbound(2, 1)])
    });
    rig.remote.fail("append", SyncError::Closed);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:append#2"), 1);
    assert!(
        rig.host
            .with(|data| data.refused.is_empty() && data.lost.is_empty())
    );
    assert_eq!(rig.host.last_state().0, SyncState::Idle);

    // The core says the feed is closed: nothing is pushed or published, it is still read.
    let mut closed = shared_work(shared_a());
    closed.closed = true;
    let mut rig = Rig::new(vec![personal_work(), closed]);
    rig.host.with(|data| {
        data.outbox.insert(shared_a(), vec![outbound(1, 1)]);
        data.candidates.insert(shared_a(), vec![doc(1)]);
    });
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_feed_outbox#2"), 0);
    assert_eq!(rig.count("h:sync_feed_snapshot_candidates#2"), 0);
    assert_eq!(rig.count("r:changes#2"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_forked_feed_is_pulled_and_exchanges_heads_but_is_never_written() {
    let mut forked = shared_work(shared_a());
    forked.forked = true;
    let mut rig = Rig::new(vec![personal_work(), forked]);
    rig.host.with(|data| {
        data.outbox.insert(shared_a(), vec![outbound(1, 1)]);
        data.candidates.insert(shared_a(), vec![doc(1)]);
        data.chunk_work.insert(
            shared_a(),
            vec![chunk_item(shared_a(), 1, FeedChunkKind::Upload)],
        );
        data.fork_detected = true;
    });
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_feed_outbox#2"), 0);
    assert_eq!(rig.count("h:sync_feed_snapshot_candidates#2"), 0);
    assert_eq!(rig.count("h:sync_feed_chunk_work#2"), 0);
    assert_eq!(rig.count("r:changes#2"), 1);
    assert_eq!(rig.count("r:put_head#2"), 1);
    assert_eq!(rig.host.last_state().0, SyncState::Forked);
}

// ----- the pull --------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_page_that_stalled_for_a_device_log_is_offered_again_after_the_logs_came() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        // The first offer stops at cursor 5 of 10; once the logs are here the same page goes through.
        data.applied.insert(
            shared_a(),
            VecDeque::from([
                SyncApplied {
                    applied: 1,
                    new_cursor: 5,
                    ..SyncApplied::default()
                },
                SyncApplied {
                    applied: 1,
                    new_cursor: 10,
                    ..SyncApplied::default()
                },
            ]),
        );
        data.needed_logs = VecDeque::from([
            Vec::new(),
            vec![LogRequest {
                account: AccountId::from_bytes([9; 16]),
                after: Some(0),
            }],
        ]);
    });
    rig.remote.with(|data| {
        data.changes
            .insert(shared_a(), VecDeque::from([page(&[5, 10], false)]));
    });
    rig.one_cycle().await;
    let pages = rig.host.with(|data| data.pages_applied.clone());
    let for_a: Vec<_> = pages
        .iter()
        .filter(|(feed, _)| *feed == shared_a())
        .collect();
    assert_eq!(for_a.len(), 2, "{pages:?}");
    assert_eq!(for_a[0].1, for_a[1].1, "the same page, offered twice");
    assert_eq!(
        rig.host.with(|data| data.foreign_logs.clone()),
        vec![AccountId::from_bytes([9; 16])]
    );
    assert_eq!(
        rig.count("r:changes#2"),
        1,
        "the page was not fetched again"
    );
    assert_eq!(rig.host.with(|data| data.cursors[&shared_a()]), 10);
}

#[tokio::test(start_paused = true)]
async fn a_page_that_stays_stalled_is_left_for_the_next_cycle() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        let stuck = SyncApplied {
            new_cursor: 5,
            ..SyncApplied::default()
        };
        data.applied
            .insert(shared_a(), VecDeque::from([stuck, stuck]));
    });
    rig.remote.with(|data| {
        data.changes
            .insert(shared_a(), VecDeque::from([page(&[5, 10], true)]));
    });
    rig.one_cycle().await;
    assert_eq!(rig.count("r:changes#2"), 1, "no endless loop");
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn pages_are_pulled_from_the_cursor_the_core_reports() {
    let mut feed = shared_work(shared_a());
    feed.cursor = 40;
    let mut rig = Rig::new(vec![personal_work(), feed]);
    rig.remote.with(|data| {
        data.changes.insert(
            shared_a(),
            VecDeque::from([page(&[41, 42], true), page(&[50], false)]),
        );
    });
    rig.one_cycle().await;
    let pages = rig.host.with(|data| data.pages_applied.clone());
    let for_a: Vec<Vec<u64>> = pages
        .into_iter()
        .filter(|(feed, _)| *feed == shared_a())
        .map(|(_, cursors)| cursors)
        .collect();
    assert_eq!(for_a, vec![vec![41, 42], vec![50]]);
    assert_eq!(rig.host.with(|data| data.cursors[&shared_a()]), 50);
}

#[tokio::test(start_paused = true)]
async fn a_pull_stuck_at_a_gap_fetches_the_snapshots_once_and_pulls_again() {
    let mut feed = shared_work(shared_a());
    feed.cursor = 3;
    let mut rig = Rig::new(vec![personal_work(), feed]);
    rig.host.with(|data| {
        data.applied.insert(
            shared_a(),
            VecDeque::from([
                SyncApplied {
                    gaps: 1,
                    new_cursor: 3,
                    ..SyncApplied::default()
                },
                SyncApplied {
                    applied: 1,
                    new_cursor: 9,
                    ..SyncApplied::default()
                },
            ]),
        );
    });
    rig.remote.with(|data| {
        data.changes.insert(
            shared_a(),
            VecDeque::from([page(&[8, 9], false), page(&[8, 9], false)]),
        );
    });
    rig.one_cycle().await;
    let steps = rig.calls(&[
        "changes",
        "sync_feed_apply",
        "snapshots",
        "sync_feed_apply_snapshots",
    ]);
    let for_a: Vec<&str> = steps
        .iter()
        .map(String::as_str)
        .filter(|call| call.ends_with("#2"))
        .collect();
    assert_eq!(
        for_a,
        vec![
            "r:changes#2",
            "h:sync_feed_apply#2",
            "r:snapshots#2",
            "h:sync_feed_apply_snapshots#2",
            "r:changes#2",
            "h:sync_feed_apply#2",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_feed_that_begins_at_cursor_zero_fetches_its_snapshots_once_per_connection() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.count("r:snapshots#2"), 1, "after the pull, at cursor 0");
    assert_eq!(rig.count("r:snapshots#1"), 1);
    // The next cycle (the pull timer) finds the feeds still at cursor 0 and does not fetch again.
    tokio::time::sleep(Duration::from_millis(PULL_INTERVAL_MS + 1_000)).await;
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:snapshots#2"), 1);
    assert_eq!(rig.count("r:snapshots#1"), 1);
}

#[tokio::test(start_paused = true)]
async fn items_still_parked_fetch_the_snapshots_but_not_every_cycle() {
    let mut feed = shared_work(shared_a());
    feed.cursor = 4;
    let mut personal = personal_work();
    personal.cursor = 4;
    let mut rig = Rig::new(vec![personal, feed]);
    rig.host.with(|data| data.parked_items = 2);
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.count("r:snapshots#2"), 1);
    tokio::time::sleep(Duration::from_millis(PULL_INTERVAL_MS + 1_000)).await;
    rig.until_cycles(2).await;
    assert_eq!(
        rig.count("r:snapshots#2"),
        1,
        "asked a minute ago: not again yet"
    );
    tokio::time::sleep(Duration::from_secs(11 * 60)).await;
    rig.until_cycles(3).await;
    assert!(
        rig.count("r:snapshots#2") >= 2,
        "asked again after the throttle"
    );
}

// ----- snapshots and marks ---------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_snapshot_that_omits_entries_is_disputed_and_the_core_is_told() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.snapshots_applied.insert(
            shared_a(),
            VecDeque::from([SnapshotsApplied {
                applied: 0,
                disputed: vec![(doc(4), 77)],
            }]),
        );
    });
    rig.one_cycle().await;
    assert_eq!(
        rig.remote.with(|data| data.disputes.clone()),
        vec![(shared_a(), doc(4), 77)]
    );
    assert_eq!(
        rig.host.with(|data| data.verified.clone()),
        vec![(shared_a(), doc(4), false)]
    );
}

#[tokio::test(start_paused = true)]
async fn the_snapshots_other_devices_published_are_fetched_checked_and_acknowledged() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.marks_wanted
            .insert(shared_a(), VecDeque::from([Some(3), None]));
        data.marks_docs.insert(shared_a(), vec![doc(6), doc(7)]);
    });
    rig.remote.with(|data| {
        data.marks.insert(
            shared_a(),
            VecDeque::from([MarksPage {
                marks: Vec::new(),
                more: true,
            }]),
        );
        data.snapshots
            .insert((shared_a(), doc(6)), b"snapshot six".to_vec());
        // The server holds nothing for doc 7: it is left unverified.
    });
    rig.one_cycle().await;
    assert_eq!(
        rig.host.with(|data| data.verified.clone()),
        vec![(shared_a(), doc(6), true)]
    );
    assert_eq!(rig.count("r:get_snapshot#2"), 2);
    // `more` asked the core again; it had nothing more to ask.
    assert_eq!(rig.count("h:sync_snapshot_marks_wanted#2"), 2);
}

#[tokio::test(start_paused = true)]
async fn the_personal_feed_asks_for_no_marks() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_snapshot_marks_wanted#1"), 0);
    assert_eq!(rig.count("h:sync_membership_pending#1"), 0);
}

// ----- chunks ----------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn chunks_have_a_budget_per_feed_and_a_full_one_asks_for_another_cycle_at_once() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        // A budget's worth of downloads in each feed, and more in the shared one.
        for feed in [personal(), shared_a()] {
            let count = if feed == personal() {
                TRANSFER_BUDGET
            } else {
                TRANSFER_BUDGET + 3
            };
            data.chunk_work.insert(
                feed,
                (0..count)
                    .map(|n| chunk_item(feed, n as u8, FeedChunkKind::Download))
                    .collect(),
            );
        }
    });
    rig.remote.with(|data| {
        for feed in [personal(), shared_a()] {
            for n in 0..(TRANSFER_BUDGET + 3) {
                data.chunks
                    .insert((feed, chunk(n as u8)), vec![n as u8; 16]);
            }
        }
    });
    rig.start();
    rig.until_cycles(1).await;
    // The first cycle moved a budget per feed and says the work is not over: a second one follows
    // without waiting for the pull interval (the paused clock moved a few milliseconds only).
    rig.until_cycles(2).await;
    let told = rig.host.with(|data| data.chunks_told.clone());
    let received = |feed: FeedId| {
        told.iter()
            .filter(|(what, other, _)| *what == "received" && *other == feed)
            .count()
    };
    assert_eq!(received(personal()), TRANSFER_BUDGET);
    assert_eq!(received(shared_a()), TRANSFER_BUDGET + 3);
    assert!(
        rig.count("h:sync_feed_chunk_work#2") >= 2,
        "a second cycle took the rest of the shared feed"
    );
}

#[tokio::test(start_paused = true)]
async fn a_quota_refusal_stops_the_uploads_of_the_feed_and_ends_in_error() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            shared_a(),
            vec![
                chunk_item(shared_a(), 1, FeedChunkKind::Upload),
                chunk_item(shared_a(), 2, FeedChunkKind::Upload),
                chunk_item(shared_a(), 3, FeedChunkKind::Download),
            ],
        );
    });
    rig.remote.fail("put_chunk", SyncError::Quota);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:put_chunk#2"), 1, "the other upload waits");
    assert_eq!(rig.count("r:get_chunk#2"), 1, "the download still runs");
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(QUOTA_FULL.to_owned()))
    );
}

#[tokio::test(start_paused = true)]
async fn deletes_come_first_and_a_copy_falls_back_to_an_upload() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            shared_a(),
            vec![
                chunk_item(shared_a(), 1, FeedChunkKind::Download),
                chunk_item(shared_a(), 2, FeedChunkKind::Copy),
                chunk_item(shared_a(), 3, FeedChunkKind::Delete),
            ],
        );
    });
    // The personal namespace does not hold chunk 2: the copy is an upload from here.
    rig.remote.fail("copy_chunk", SyncError::NotFound);
    rig.one_cycle().await;
    let steps = rig.calls(&[
        "delete_chunk",
        "copy_chunk",
        "put_chunk",
        "get_chunk",
        "sync_feed_chunk_deleted",
        "sync_feed_chunk_uploaded",
        "sync_feed_chunk_unavailable",
    ]);
    assert_eq!(
        steps,
        vec![
            "r:delete_chunk#2",
            "h:sync_feed_chunk_deleted#2",
            "r:copy_chunk#2",
            "r:put_chunk#2",
            "h:sync_feed_chunk_uploaded#2",
            "r:get_chunk#2",
            "h:sync_feed_chunk_unavailable#2",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_shared_feed_that_refuses_writes_still_serves_downloads() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            shared_a(),
            vec![
                chunk_item(shared_a(), 1, FeedChunkKind::Upload),
                chunk_item(shared_a(), 2, FeedChunkKind::Upload),
                chunk_item(shared_a(), 3, FeedChunkKind::Download),
            ],
        );
    });
    rig.remote.fail("put_chunk", SyncError::Forbidden);
    rig.remote
        .with(|data| drop(data.chunks.insert((shared_a(), chunk(3)), vec![3; 8])));
    rig.one_cycle().await;
    assert_eq!(rig.count("r:put_chunk#2"), 1);
    assert_eq!(rig.count("r:get_chunk#2"), 1);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

// ----- failures --------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_refused_signature_stops_one_feed_and_the_others_are_still_worked() {
    let (_, profile) = membership_entries();
    let mut rig = Rig::new(vec![
        personal_work(),
        shared_work(shared_a()),
        shared_work(shared_b()),
    ]);
    // The server refuses the signature of feed A's membership entry.
    rig.host
        .with(|data| data.membership.insert(shared_a(), vec![profile]));
    rig.remote
        .fail("membership_append", SyncError::Authentication);
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_feed_head#2"), 0, "feed A stopped");
    assert_eq!(rig.count("h:sync_feed_head#3"), 1, "feed B was worked");
    assert_eq!(
        rig.count("h:sync_feed_head#1"),
        1,
        "so was the personal feed"
    );
    let (state, message) = rig.host.last_state();
    assert_eq!(state, SyncState::Error);
    assert!(message.is_some_and(|message| !message.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_core_crypto_failure_on_a_shared_feed_stops_that_feed_only() {
    let mut rig = Rig::new(vec![
        personal_work(),
        shared_work(shared_a()),
        shared_work(shared_b()),
    ]);
    // An item of feed A does not verify.
    rig.host.fail_feed(
        "sync_feed_apply",
        shared_a(),
        ApiError::crypto("an item does not verify"),
    );
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_feed_head#2"), 0, "feed A stopped");
    assert_eq!(rig.count("h:sync_feed_head#3"), 1, "feed B was worked");
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some("an item does not verify".to_owned()))
    );
}

#[tokio::test(start_paused = true)]
async fn any_other_core_failure_ends_the_cycle() {
    let mut rig = Rig::new(vec![
        personal_work(),
        shared_work(shared_a()),
        shared_work(shared_b()),
    ]);
    rig.host.fail_feed(
        "sync_feed_apply",
        shared_a(),
        ApiError::storage("disk full"),
    );
    rig.one_cycle().await;
    assert_eq!(
        rig.count("h:sync_feed_head#3"),
        0,
        "the cycle ended at feed A"
    );
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some("disk full".to_owned()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_failure_of_the_personal_feed_ends_the_cycle_before_the_shared_feeds() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.remote
        .fail("changes", SyncError::Protocol("broken".to_owned()));
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_feed_head#2"), 0);
    assert_eq!(rig.host.last_state().0, SyncState::Error);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_server_is_offline_and_retried_with_a_growing_delay() {
    let mut rig = Rig::new(vec![personal_work()]);
    for _ in 0..3 {
        rig.remote
            .fail("server_info", SyncError::Unavailable("down".to_owned()));
    }
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.host.last_state().0, SyncState::Offline);
    let started = tokio::time::Instant::now();
    rig.until_cycles(2).await;
    assert_eq!(rig.host.last_state().0, SyncState::Offline);
    let first_delay = started.elapsed();
    let after_second = tokio::time::Instant::now();
    rig.until_cycles(3).await;
    let second_delay = after_second.elapsed();
    assert!(
        second_delay >= first_delay,
        "the delay grows: {first_delay:?} then {second_delay:?}"
    );
    rig.until_cycles(4).await;
    assert_eq!(
        rig.host.last_state().0,
        SyncState::Idle,
        "the server came back"
    );
}

#[tokio::test(start_paused = true)]
async fn a_429_is_unavailable_never_quota() {
    // The mapping lives in the HTTP remote; here the driver's side: an `Unavailable` ends the cycle
    // as `Offline` (the user is not told the quota is full).
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote
        .fail("changes", SyncError::Unavailable("slow down".to_owned()));
    rig.start();
    rig.until_cycles(1).await;
    let (state, message) = rig.host.last_state();
    assert_eq!(state, SyncState::Offline);
    assert_ne!(message.as_deref(), Some(QUOTA_FULL));
}

#[tokio::test(start_paused = true)]
async fn a_refused_session_authenticates_again_and_retries_once() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host
        .with(|data| data.outbox.insert(shared_a(), vec![outbound(1, 1)]));
    rig.remote.fail("append", SyncError::Unauthorized);
    // The 401 drops the session of the remote (the fake does it), so the driver authenticates again.
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.count("r:append#2"), 2, "refused, then retried");
    assert_eq!(
        rig.remote.with(|data| data.authentications),
        2,
        "once at the start, once after the 401"
    );
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_server_that_speaks_another_protocol_is_an_error() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| data.protocol = 1);
    rig.one_cycle().await;
    let (state, message) = rig.host.last_state();
    assert_eq!(state, SyncState::Error);
    assert!(message.is_some_and(|message| message.contains("protocol")));
    assert_eq!(rig.count("h:sync_feeds"), 0);
}

#[tokio::test(start_paused = true)]
async fn an_account_the_server_does_not_know_asks_the_core_for_a_token() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| data.log = None);
    rig.one_cycle().await;
    assert_eq!(rig.host.with(|data| data.unknown_asked), 1);
    assert_eq!(rig.count("h:sync_feeds"), 0, "the cycle stopped there");

    // With a token the account is created and the log is marked published.
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| data.log = None);
    rig.host.with(|data| data.token = Some("dev".to_owned()));
    rig.one_cycle().await;
    assert_eq!(rig.count("r:create_account"), 1);
    assert_eq!(rig.host.with(|data| data.published), 1);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn the_unpublished_tail_of_the_device_log_is_published() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.log = vec![b"genesis".to_vec(), b"added".to_vec()];
        data.published = 1;
    });
    rig.one_cycle().await;
    assert_eq!(rig.remote.with(|data| data.log.clone().unwrap().len()), 2);
    assert_eq!(rig.host.with(|data| data.published), 2);
}

#[tokio::test(start_paused = true)]
async fn a_co_members_log_that_does_not_verify_is_noted_and_the_others_are_handed_over() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| {
        data.needed_logs = VecDeque::from([vec![
            LogRequest {
                account: AccountId::from_bytes([8; 16]),
                after: None,
            },
            LogRequest {
                account: AccountId::from_bytes([9; 16]),
                after: None,
            },
        ]]);
    });
    rig.host
        .fail("sync_foreign_log_received", ApiError::crypto("forked log"));
    rig.one_cycle().await;
    assert_eq!(
        rig.host.with(|data| data.foreign_logs.clone()),
        vec![AccountId::from_bytes([9; 16])]
    );
    assert_eq!(rig.count("h:sync_feed_head#2"), 1);
    let (state, message) = rig.host.last_state();
    assert_eq!(state, SyncState::Error);
    assert_eq!(message.as_deref(), Some("forked log"));
}

#[tokio::test(start_paused = true)]
async fn a_locked_core_pauses_the_driver_without_a_state() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.fail("sync_feeds", ApiError::locked());
    rig.start();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(rig.host.with(|data| {
        data.states
            .iter()
            .all(|(state, _)| matches!(state, SyncState::Pushing | SyncState::Pulling))
    }));
    assert_eq!(
        rig.host
            .with(|data| data.states.first().map(|(state, _)| *state)),
        Some(SyncState::Pushing)
    );
}

// ----- triggers --------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_sync_request_runs_a_full_cycle_after_the_debounce() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.start();
    rig.until_cycles(1).await;
    rig.host
        .with(|data| data.outbox.insert(shared_a(), vec![outbound(1, 1)]));
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:append#2"), 1);
    assert_eq!(rig.count("h:sync_feed_pushed#2"), 1);
}

#[tokio::test(start_paused = true)]
async fn the_pull_timer_pushes_only_what_the_core_says_is_pending() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.start();
    rig.until_cycles(1).await;
    tokio::time::sleep(Duration::from_millis(PULL_INTERVAL_MS + 500)).await;
    rig.until_cycles(2).await;
    assert_eq!(
        rig.count("h:sync_feed_outbox#2"),
        1,
        "the first cycle was a full one"
    );
    rig.host.with(|data| {
        data.pending_entries = 1;
        data.outbox.insert(shared_a(), vec![outbound(1, 1)]);
    });
    tokio::time::sleep(Duration::from_millis(PULL_INTERVAL_MS + 500)).await;
    rig.until_cycles(3).await;
    assert_eq!(
        rig.count("h:sync_feed_outbox#2"),
        2,
        "pending entries are pushed by a pull cycle"
    );
}

#[tokio::test(start_paused = true)]
async fn an_unconfigured_core_idles_the_driver() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.configured = false);
    rig.start();
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(rig.count("r:server_info"), 0);
    // Turning sync on starts a cycle.
    rig.host.with(|data| data.configured = true);
    rig.host.emit(&Event::SyncChanged);
    rig.until_cycles(1).await;
    assert_eq!(rig.count("r:server_info"), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_handle_stops_the_loop_and_unsubscribes() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;
    let handle = rig._handle.take().unwrap();
    assert!(handle.is_running());
    drop(handle);
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(rig.host.cycles(), 1, "no cycle after the stop");
}

// ----- scheduling, reporting and the session (carried over from the tests of protocol v1's driver) ---

/// Lets the paused clock run, a millisecond at a time, until `condition` holds.
async fn until(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..400_000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("never happened: {what}");
}

#[test]
fn the_backoff_doubles_from_one_second_to_five_minutes_and_resets() {
    let mut backoff = Backoff::new();
    let delays: Vec<u64> = (0..11).map(|_| backoff.next_delay().as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
    assert_eq!(BACKOFF_MIN, Duration::from_secs(1));
    assert_eq!(BACKOFF_MAX, Duration::from_secs(300));
    backoff.reset();
    assert_eq!(backoff.next_delay(), BACKOFF_MIN);
}

#[tokio::test(start_paused = true)]
async fn requests_are_debounced_into_one_cycle() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;
    rig.remote.with(|data| data.times.clear());

    let mut last = Instant::now();
    for _ in 0..4 {
        rig.host.emit(&Event::SyncRequested);
        last = Instant::now();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(390)).await;
    assert!(
        rig.remote.times("server_info").is_empty(),
        "still within the debounce"
    );

    rig.until_cycles(2).await;
    let started = rig.remote.times("server_info");
    assert_eq!(started.len(), 1, "one cycle for the whole burst");
    assert_eq!(started[0] - last, Duration::from_millis(500));
}

#[tokio::test(start_paused = true)]
async fn a_steady_stream_of_requests_cannot_delay_a_push_forever() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;
    rig.remote.with(|data| data.times.clear());

    let first = Instant::now();
    for _ in 0..30 {
        rig.host.emit(&Event::SyncRequested);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let started = rig.remote.times("server_info");
    assert!(
        !started.is_empty(),
        "no cycle during nine seconds of requests"
    );
    assert_eq!(started[0] - first, MAX_DEBOUNCE);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_server_means_offline_and_exponential_backoff() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote
        .fail_always("server_info", &SyncError::Unavailable("down".into()));
    rig.start();
    until("five attempts", || {
        rig.remote.times("server_info").len() >= 5
    })
    .await;
    let times = rig.remote.times("server_info");
    let gaps: Vec<u64> = times.windows(2).map(|w| (w[1] - w[0]).as_secs()).collect();
    assert_eq!(gaps[..4], [1, 2, 4, 8]);
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Offline, Some("down".into()))
    );

    // The server comes back: the next attempt succeeds.
    rig.remote.heal("server_info");
    until("a clean cycle", || {
        rig.host.last_state().0 == SyncState::Idle
    })
    .await;

    // A later failure starts the backoff over at one second.
    rig.remote.with(|data| data.times.clear());
    rig.remote
        .fail_always("server_info", &SyncError::Unavailable("down".into()));
    rig.host.emit(&Event::SyncRequested);
    until("two attempts", || {
        rig.remote.times("server_info").len() >= 2
    })
    .await;
    let times = rig.remote.times("server_info");
    assert_eq!(times[1] - times[0], BACKOFF_MIN);
}

#[tokio::test(start_paused = true)]
async fn requests_while_offline_wait_for_the_backoff() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote
        .fail_always("server_info", &SyncError::Unavailable("down".into()));
    rig.start();
    until("three attempts", || {
        rig.remote.times("server_info").len() >= 3
    })
    .await;
    // The next attempt is 4 s away; edits in between do not shorten it.
    let third = *rig.remote.times("server_info").last().unwrap();
    rig.host.emit(&Event::SyncRequested);
    until("four attempts", || {
        rig.remote.times("server_info").len() >= 4
    })
    .await;
    let fourth = *rig.remote.times("server_info").last().unwrap();
    assert_eq!(fourth - third, Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn a_device_without_a_kit_is_told_to_create_one() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.identity = None);
    rig.one_cycle().await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(NO_KIT.to_owned()))
    );
    let remote_calls: Vec<String> = rig
        .log
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.starts_with("r:"))
        .cloned()
        .collect();
    assert_eq!(remote_calls, ["r:server_info"]);
}

#[tokio::test(start_paused = true)]
async fn a_refused_invite_token_is_reported() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| data.log = None);
    rig.host.with(|data| data.token = Some("wrong".to_owned()));
    rig.remote.fail("create_account", SyncError::Unauthorized);
    rig.one_cycle().await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(INVITE_TOKEN_REFUSED.to_owned()))
    );
    assert_eq!(
        rig.host.with(|data| data.unknown_asked),
        1,
        "the status asks for another token"
    );
    assert_eq!(rig.count("h:sync_feeds"), 0, "the cycle stopped there");
}

#[tokio::test(start_paused = true)]
async fn an_invite_token_survives_a_server_that_is_down() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| data.log = None);
    rig.host.with(|data| data.token = Some("invite".to_owned()));
    rig.remote
        .fail("create_account", SyncError::Unavailable("down".into()));
    rig.start();
    until("the account exists", || {
        rig.remote.with(|data| data.log.is_some())
    })
    .await;
    // The core handed the token out once; the driver kept it for the retry.
    assert_eq!(rig.count("r:create_account"), 2);
    assert_eq!(rig.host.with(|data| data.unknown_asked), 0);
}

#[tokio::test(start_paused = true)]
async fn an_account_the_server_lost_between_two_calls_is_created_again() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.log = vec![b"genesis".to_vec(), b"added".to_vec()];
        data.published = 1;
        data.token = Some("invite".to_owned());
    });
    rig.remote.fail("append_log", SyncError::NotFound);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:create_account"), 1);
    assert_eq!(rig.remote.with(|data| data.log.clone().unwrap().len()), 2);
    assert_eq!(rig.host.with(|data| data.published), 2);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn the_servers_log_reaches_the_core_before_ours_is_published() {
    // Another device joined through the kit: the server's log is one entry longer.
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| {
        data.log = Some(vec![b"genesis".to_vec(), b"other device added".to_vec()]);
    });
    rig.one_cycle().await;
    let calls = rig.calls(&["sync_log_received", "sync_account_log", "authenticate"]);
    assert_eq!(
        calls,
        [
            "h:sync_log_received",
            "h:sync_account_log",
            "r:authenticate"
        ],
        "{calls:?}"
    );
    assert_eq!(
        rig.count("r:append_log"),
        0,
        "nothing of ours is missing on the server"
    );
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_server_log_that_does_not_extend_ours_is_an_error() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.fail(
        "sync_log_received",
        ApiError::crypto("the server's device log does not match this device's"),
    );
    rig.one_cycle().await;
    assert_eq!(
        rig.host.last_state(),
        (
            SyncState::Error,
            Some("the server's device log does not match this device's".to_owned())
        )
    );
    assert_eq!(rig.count("r:authenticate"), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_server_address_recreates_the_remote() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;

    rig.host.with(|data| data.server_url = OTHER_URL.to_owned());
    rig.host.emit(&Event::SyncChanged);
    rig.until_cycles(2).await;
    assert_eq!(*rig.urls.lock().unwrap(), [URL, OTHER_URL]);
}

#[tokio::test(start_paused = true)]
async fn disabling_sync_idles_the_driver_and_enabling_starts_it() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;

    rig.host.with(|data| data.configured = false);
    rig.host.emit(&Event::SyncChanged);
    rig.host.emit(&Event::SyncRequested);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(rig.count("r:server_info"), 1, "no cycle while disabled");

    rig.host.with(|data| data.configured = true);
    rig.host.emit(&Event::SyncChanged);
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:server_info"), 2);
}

#[tokio::test(start_paused = true)]
async fn an_unconfigured_device_never_calls_the_remote() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.configured = false);
    rig.start();
    rig.host.emit(&Event::SyncRequested);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(rig.calls(&["server_info"]).is_empty());
    assert!(rig.urls.lock().unwrap().is_empty());
    assert!(rig.host.with(|data| data.states.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn stopping_the_driver_unsubscribes_and_ends_the_loop() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.host.listener_count(), 1);
    let handle = rig._handle.take().unwrap();
    assert!(handle.is_running());

    handle.stop();
    assert_eq!(rig.host.listener_count(), 0);
    rig.host.emit(&Event::SyncRequested);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(rig.count("r:server_info"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_core_that_cannot_sign_is_reported_rather_than_the_server() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| data.sign = true);
    rig.host
        .fail("sync_sign", ApiError::crypto("no device key"));
    rig.one_cycle().await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some("no device key".to_owned()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_refused_device_is_an_error_without_backoff() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.fail("authenticate", SyncError::Unauthorized);
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(SyncError::Unauthorized.to_string()))
    );
    // The next request retries after the usual debounce, not after a backoff.
    let requested = Instant::now();
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    let started = rig.remote.times("server_info")[1];
    assert_eq!(started - requested, Duration::from_millis(500));
}

#[tokio::test(start_paused = true)]
async fn a_device_refused_as_idle_is_told_to_pair_again() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.fail_always("authenticate", &SyncError::Idle);
    rig.one_cycle().await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(IDLE_DEVICE.to_owned()))
    );
    assert_eq!(rig.count("r:authenticate"), 1, "no second attempt");
}

#[tokio::test(start_paused = true)]
async fn an_idle_refusal_while_renewing_a_session_is_final_too() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host
        .with(|data| data.outbox.insert(personal(), vec![outbound(1, 1)]));
    // The first cycle's session expires on `append` and is renewed (the device is not idle yet):
    // the entry goes up.
    rig.remote.fail("append", SyncError::Unauthorized);
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.host.with(|data| data.pushed.len()), 1);

    // Later the server forgot the session and the device has been idle too long.
    rig.remote.with(|data| data.authenticated = false);
    rig.remote.fail_always("authenticate", &SyncError::Idle);
    rig.host
        .with(|data| data.outbox.insert(personal(), vec![outbound(2, 1)]));
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(IDLE_DEVICE.to_owned()))
    );
    assert_eq!(rig.count("r:authenticate"), 3, "one per refusal, no loop");
}

#[tokio::test(start_paused = true)]
async fn a_second_refusal_is_final() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.fail_always("heads", &SyncError::Unauthorized);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:authenticate"), 2, "one renewal");
    assert_eq!(rig.count("r:heads#1"), 2, "one retry");
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(SyncError::Unauthorized.to_string()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_refusal_that_keeps_the_session_is_final_at_once() {
    // A 403: another account or device, a bad signature. A new session would not help.
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host
        .with(|data| data.outbox.insert(personal(), vec![outbound(1, 1)]));
    rig.remote.with(|data| data.keep_session = true);
    rig.remote.fail("append", SyncError::Unauthorized);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:authenticate"), 1);
    assert_eq!(rig.count("r:append#1"), 1);
    assert_eq!(rig.host.last_state().0, SyncState::Error);
}

#[tokio::test(start_paused = true)]
async fn a_held_session_is_reused_across_cycles() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:authenticate"), 1);
    assert_eq!(rig.count("r:server_info"), 2);
}

#[tokio::test(start_paused = true)]
async fn the_recovery_blob_goes_up_after_authentication_and_once() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.recovery_blob = Some(([6; 32], b"wrapped".to_vec()));
    });
    rig.start();
    rig.until_cycles(1).await;
    let calls = rig.calls(&["authenticate", "recovery_put"]);
    assert_eq!(calls, ["r:authenticate", "r:recovery_put"], "{calls:?}");
    assert_eq!(
        rig.remote.with(|data| data.recovery.clone()),
        [([6; 32], b"wrapped".to_vec())]
    );
    assert_eq!(rig.count("h:sync_recovery_published"), 1);

    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:recovery_put"), 1, "published once");
}

#[tokio::test(start_paused = true)]
async fn voided_recovery_blobs_leave_the_server() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.recovery_blob = Some(([6; 32], b"new kit".to_vec()));
        data.recovery_obsolete = vec![[1; 32], [2; 32]];
    });
    rig.remote
        .with(|data| data.recovery = vec![([1; 32], b"old kit".to_vec())]);
    rig.one_cycle().await;
    rig.remote.with(|data| {
        assert_eq!(data.recovery_deletes, [[1; 32], [2; 32]]);
        assert_eq!(data.recovery, [([6; 32], b"new kit".to_vec())]);
    });
    assert_eq!(rig.count("h:sync_recovery_deleted"), 2);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_the_server_keeps_a_newer_one_of_is_skipped_without_failing_the_cycle() {
    // v1 counted such a `Conflict` as published; v2 leaves the document a candidate (the pull
    // teaches the core the newer snapshot) and goes on.
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host
        .with(|data| data.candidates.insert(personal(), vec![doc(1)]));
    rig.remote
        .fail("put_snapshot", SyncError::Conflict("covers less".into()));
    rig.one_cycle().await;
    assert_eq!(rig.count("r:put_snapshot#1"), 1);
    assert_eq!(rig.count("h:sync_feed_snapshot_published#1"), 0);
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_page_that_promises_more_without_advancing_is_refused() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.remote.with(|data| {
        data.changes.insert(
            personal(),
            VecDeque::from(vec![page(&[], true), page(&[], true), page(&[], true)]),
        );
    });
    rig.one_cycle().await;
    let (state, message) = rig.host.last_state();
    assert_eq!(state, SyncState::Error);
    assert!(message.unwrap().contains("more changes"));
    assert_eq!(rig.count("r:changes#1"), 1);
}

#[tokio::test(start_paused = true)]
async fn the_heads_that_disagree_end_the_cycle_forked() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.fork_detected = true);
    rig.one_cycle().await;
    assert_eq!(rig.host.last_state().0, SyncState::Forked);
}

#[tokio::test(start_paused = true)]
async fn only_the_first_eight_hex_characters_of_a_hash_reach_a_log() {
    // The driver logs counts and cursors; a chunk hash, when it is named at all, is cut to eight.
    let hash = ChunkId::from_bytes([0xab; 32]);
    let mut hex = hash.to_hex();
    hex.truncate(8);
    assert_eq!(hex, "abababab");
}

// ----- attachment chunks (carried over from the tests of protocol v1's driver) --------------------------

fn chunk_told(rig: &Rig, what: &str) -> Vec<ChunkId> {
    rig.host.with(|data| {
        data.chunks_told
            .iter()
            .filter(|(told, _, _)| *told == what)
            .map(|(_, _, hash)| *hash)
            .collect()
    })
}

#[tokio::test(start_paused = true)]
async fn a_budget_of_chunks_the_server_lacks_still_counts_as_used() {
    // Every answer is "not yet": the core defers each, so the next ones are worth asking at once.
    let mut rig = Rig::new(vec![personal_work()]);
    let count = u8::try_from(TRANSFER_BUDGET + 2).unwrap();
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            (1..=count)
                .map(|n| chunk_item(personal(), n, FeedChunkKind::Download))
                .collect(),
        );
    });
    rig.start();
    rig.until_cycles(2).await;
    let started = rig.remote.times("server_info");
    assert_eq!(started[1], started[0], "no pull interval in between");
    assert_eq!(chunk_told(&rig, "unavailable").len(), usize::from(count));
    assert!(chunk_told(&rig, "received").is_empty());
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_chunk_the_server_does_not_hold_yet_is_left_for_later() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            vec![
                chunk_item(personal(), 1, FeedChunkKind::Download),
                chunk_item(personal(), 2, FeedChunkKind::Download),
            ],
        );
    });
    rig.remote
        .with(|data| data.chunks.insert((personal(), chunk(2)), vec![2; 16]));
    rig.one_cycle().await;
    assert_eq!(chunk_told(&rig, "unavailable"), [chunk(1)]);
    assert_eq!(chunk_told(&rig, "received"), [chunk(2)]);
    assert_eq!(rig.count("r:get_chunk#1"), 2);
    assert_eq!(rig.host.last_state(), (SyncState::Idle, None));
}

#[tokio::test(start_paused = true)]
async fn an_upload_whose_chunk_is_gone_here_is_skipped() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.chunk_gone.push(chunk(1));
        data.chunk_work.insert(
            personal(),
            vec![
                chunk_item(personal(), 1, FeedChunkKind::Upload),
                chunk_item(personal(), 2, FeedChunkKind::Upload),
            ],
        );
    });
    rig.one_cycle().await;
    assert_eq!(
        rig.calls(&[
            "sync_feed_chunk_bytes",
            "put_chunk",
            "sync_feed_chunk_uploaded"
        ]),
        [
            "h:sync_feed_chunk_bytes#1",
            "h:sync_feed_chunk_bytes#1",
            "r:put_chunk#1",
            "h:sync_feed_chunk_uploaded#1",
        ]
    );
    assert_eq!(
        rig.remote.with(|data| data.put_chunks.clone()),
        [(personal(), chunk(2))]
    );
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_server_in_the_middle_of_a_batch_is_resumed_by_the_next_cycle() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            (1..=4)
                .map(|n| chunk_item(personal(), n, FeedChunkKind::Download))
                .collect(),
        );
    });
    rig.remote.with(|data| {
        for n in 1..=4 {
            data.chunks.insert((personal(), chunk(n)), vec![n; 16]);
        }
    });
    rig.remote
        .fail_nth("get_chunk", 1, SyncError::Unavailable("down".into()));
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Offline, Some("down".to_owned()))
    );
    assert_eq!(
        chunk_told(&rig, "received"),
        [chunk(1)],
        "what came is kept"
    );
    assert_eq!(rig.count("r:put_head#1"), 0, "the cycle stopped there");

    // The usual backoff, then the rest of the batch.
    rig.until_cycles(2).await;
    let started = rig.remote.times("server_info");
    assert_eq!(started[1] - started[0], BACKOFF_MIN);
    assert_eq!(
        chunk_told(&rig, "received"),
        [chunk(1), chunk(2), chunk(3), chunk(4)],
        "each chunk once"
    );
    assert_eq!(rig.count("r:get_chunk#1"), 5);
    assert_eq!(rig.host.last_state(), (SyncState::Idle, None));
}

#[tokio::test(start_paused = true)]
async fn a_core_error_during_the_transfers_ends_the_cycle_with_its_message() {
    // A chunk whose bytes do not match its hash: the core refuses it.
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            (1..=2)
                .map(|n| chunk_item(personal(), n, FeedChunkKind::Download))
                .collect(),
        );
    });
    rig.remote.with(|data| {
        for n in 1..=2 {
            data.chunks.insert((personal(), chunk(n)), vec![n; 16]);
        }
    });
    rig.host.fail(
        "sync_feed_chunk_received",
        ApiError::crypto("an attachment chunk does not match its address"),
    );
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(
        rig.host.last_state(),
        (
            SyncState::Error,
            Some("an attachment chunk does not match its address".to_owned())
        )
    );
    assert_eq!(rig.count("r:get_chunk#1"), 1, "the step stopped");
    assert_eq!(rig.count("h:sync_feed_head#1"), 0);

    // No backoff: the next trigger tries again.
    let requested = Instant::now();
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(
        rig.remote.times("server_info")[1] - requested,
        Duration::from_millis(500)
    );
}

#[tokio::test(start_paused = true)]
async fn a_refused_upload_other_than_the_quota_ends_the_cycle() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            vec![
                chunk_item(personal(), 1, FeedChunkKind::Upload),
                chunk_item(personal(), 2, FeedChunkKind::Download),
            ],
        );
    });
    rig.remote
        .with(|data| data.chunks.insert((personal(), chunk(2)), vec![2; 16]));
    let refusal = SyncError::Protocol("the chunk does not match its hash".to_owned());
    rig.remote.fail("put_chunk", refusal.clone());
    rig.one_cycle().await;
    assert_eq!(
        rig.host.last_state(),
        (SyncState::Error, Some(refusal.to_string()))
    );
    assert_eq!(rig.count("r:get_chunk#1"), 0);
    assert!(chunk_told(&rig, "uploaded").is_empty());
    assert_eq!(
        rig.host
            .with(|data| data.chunk_work.get(&personal()).map(Vec::len)),
        Some(2),
        "both still to do"
    );
}

#[tokio::test(start_paused = true)]
async fn an_expired_session_during_a_chunk_transfer_is_renewed_once() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            vec![
                chunk_item(personal(), 1, FeedChunkKind::Delete),
                chunk_item(personal(), 2, FeedChunkKind::Upload),
                chunk_item(personal(), 3, FeedChunkKind::Download),
            ],
        );
    });
    rig.remote
        .with(|data| data.chunks.insert((personal(), chunk(3)), vec![3; 16]));
    for call in ["delete_chunk", "put_chunk", "get_chunk"] {
        rig.remote.fail(call, SyncError::Unauthorized);
    }
    rig.one_cycle().await;
    assert_eq!(
        rig.count("r:authenticate"),
        4,
        "the first session and one renewal per refusal"
    );
    for call in ["r:delete_chunk#1", "r:put_chunk#1", "r:get_chunk#1"] {
        assert_eq!(rig.count(call), 2, "{call} retried once");
    }
    assert_eq!(chunk_told(&rig, "deleted"), [chunk(1)]);
    assert_eq!(chunk_told(&rig, "uploaded"), [chunk(2)]);
    assert_eq!(chunk_told(&rig, "received"), [chunk(3)]);
    assert_eq!(rig.host.last_state(), (SyncState::Idle, None));
}

#[tokio::test(start_paused = true)]
async fn a_sync_request_still_triggers_a_cycle_that_carries_new_chunks() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;

    // An attachment finished importing: the core queues its chunk and asks for a cycle.
    rig.host.with(|data| {
        data.chunk_work.insert(
            personal(),
            vec![chunk_item(personal(), 9, FeedChunkKind::Upload)],
        );
    });
    let requested = Instant::now();
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(
        rig.remote.times("server_info")[1] - requested,
        Duration::from_millis(500)
    );
    assert_eq!(
        rig.remote.with(|data| data.put_chunks.clone()),
        [(personal(), chunk(9))]
    );
}

#[tokio::test(start_paused = true)]
async fn a_server_that_lost_its_data_is_bootstrapped_again() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.count("r:snapshots#1"), 1);

    // A new epoch: the core starts over, so the feed begins at cursor 0 once more.
    rig.host.with(|data| data.epoch_reset = true);
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:snapshots#1"), 2);
}

#[tokio::test(start_paused = true)]
async fn a_push_stops_at_the_first_refused_entry() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.outbox.insert(
            personal(),
            vec![outbound(1, 1), outbound(2, 1), outbound(3, 1)],
        );
    });
    rig.remote
        .fail_nth("append", 1, SyncError::Conflict("duplicate".into()));
    rig.one_cycle().await;
    assert_eq!(rig.host.with(|data| data.pushed.len()), 1);
    assert_eq!(rig.count("r:append#1"), 2, "the third entry waits");
}
