//! The sync driver and its seams for protocol v2 (M13 slice 1, ADR-0045 section 9,
//! `docs/development/milestone-13.md` section 1.6): [`SyncRemote`], the `Send` view of
//! `kb_sync::v2::RemoteStore` the driver runs against, [`SyncHost`], what the driver needs from the
//! core (one to one with `kb_core::Core`'s per-feed methods, see `crates/kb-core/src/feeds.rs`
//! and `membership.rs`), and [`SyncDriver`], the loop.
//!
//! Protocol v1 is retired (ADR-0045: v2 replaces it without a server migration): `HttpRemote` and the
//! reference `MemoryRemote` implement [`SyncRemote`], and the shell implements this module's
//! [`SyncHost`] (one session, every feed). The path `kb_sync::v2` of the protocol's API stays.
//!
//! # The cycle (ADR-0045 section 9), one session per device, each feed with its own transfer budget
//!
//! 1. The server's epoch (`server_info`), the account's log, authentication; the core resets every
//!    feed on a new epoch (`sync_remote_epoch`).
//! 2. `sync_feeds` lists the feeds (every cycle: it also finalises a `Leave` the server took); the
//!    personal feed comes first: outbox, snapshots, pull, heads, chunks.
//! 3. The co-members' logs the core asks for (`sync_needed_logs`, `logs_batch`,
//!    `sync_foreign_log_received`), so the shared feeds can be judged.
//! 4. Each shared feed of `sync_feeds()` (listed again, so that a space the account document just
//!    taught is worked at once):
//!    1. the pending membership entries by compare-and-swap (`sync_membership_pending`: the `Create`,
//!       `seq` 0, through `feeds_create`, the rest through `membership_append`;
//!       `sync_membership_posted`; a 409 `conflict` is `sync_membership_conflict` and the next
//!       cycle retries), then the membership log read directly when the core wants it
//!       (`sync_membership_wanted`, `membership_get`, `sync_membership_fetched`, once unless the page
//!       says `more`);
//!    2. keyring and document entries (`sync_feed_outbox`, `append`, `sync_feed_pushed`): a 409
//!       `rekey_needed` is `sync_feed_rekey_needed` and ends the uploads of that feed (the core mints
//!       the Rekey when it is the next writer), a 409 `stale_epoch` ends them (the pull teaches the
//!       new epoch, the next outbox is resealed), a 403 `forbidden` is `sync_feed_refused` (the pull
//!       still runs), a 409 `feed_closed` ends them, a 404 is `sync_feed_lost` (the space becomes an
//!       archive);
//!    3. snapshots to publish (`sync_feed_snapshot_*`, `put_snapshot`);
//!    4. the pull: `changes` pages through `sync_feed_apply`; a page that stalled for a co-member's
//!       log is offered again once after the logs were fetched; a feed that started at cursor 0, a
//!       pull that saw cursor gaps and items still parked ask for the feed's snapshots
//!       (`sync_feed_apply_snapshots`, a `snapshot_dispute` for each `disputed` answer);
//!    5. the snapshots other devices published (`sync_snapshot_marks_wanted`, `snapshot_marks`,
//!       `get_snapshot`, `sync_snapshot_verified`);
//!    6. chunks (`sync_feed_chunk_*`: upload, download, delete, copy), then heads (`sync_feed_head`,
//!       `put_head`, `heads`, `sync_feed_heads_received`).
//!
//! Failures that concern one shared feed (a refused signature, a protocol violation, a quota that
//! is full) stop that feed's step only; the others are still worked and the cycle ends in `Error` with
//! the first message. `Unavailable` (429 included, never `Quota`), an expired idle device and every core
//! error end the cycle (`Offline` with a backoff of 1 s to 5 min, or `Error`).
//!
//! The realtime socket (M13 slice 3, ADR-0050, [`crate::realtime`]): the remote opens one WebSocket per
//! device after the session opens ([`SyncRemote::realtime`]); the driver subscribes it to every feed of
//! `sync_feeds()` (before the feeds are pulled, and again when the list changes), applies the entries it
//! relays through [`SyncHost::sync_feed_apply_inline`], pulls the one feed a `changed` frame names (or an
//! entry the core could not apply from where it is) outside the cycle, runs a pull cycle when the
//! socket (re)connects, hands presence frames to [`SyncHost::presence_received`] and asks
//! [`SyncHost::presence_expire`] every five seconds. A cycle in which any feed's chunk transfers used
//! the whole transfer budget ([`kb_types::sync::TRANSFER_BUDGET`]) is followed at once by another.

use std::future::Future;
use std::pin::Pin;

use kb_crypto::Signature;
use kb_sync::v2::{
    FeedChunkWork, FeedCreated, FeedListing, FeedWork, InlineEntry, InlineOutcome, InviteOpened,
    LinkOutcome, LinkPut, LinkWork, LogChunk, LogRequest, MarksPage, MembershipHead,
    MembershipPage, OutboundEntry, ServerInfo, SnapshotMark, SnapshotsApplied, SyncApplied,
};
use kb_sync::{ChangesPage, PairingPage, SnapshotsPage, SyncError};
use kb_types::sync::PUSH_DEBOUNCE_MS;
use kb_types::{
    AccountId, ApiError, ChunkId, DeviceId, DocId, Event, FeedId, InviteId, LinkId, SyncInfo,
    SyncState,
};

use crate::RecoveryUpload;
use crate::http::AuthSigner;
use crate::realtime::Realtime;

/// A boxed, `Send` remote call. `kb_sync::v2::RemoteStore` has `async fn`s without a `Send` bound (it
/// must work on wasm32), so a task on tokio's multi-threaded runtime cannot hold its futures
/// generically; [`SyncRemote`] boxes `Send` futures for the two remotes the driver runs against.
pub type RemoteFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SyncError>> + Send + 'a>>;

/// Runs a remote call that needs the session (`$call` is evaluated again for the retry). When the
/// remote refused it and dropped its session (a 401: expired, or the server forgot it), authenticates
/// once and retries once; a refusal that leaves the session in place (a 403: wrong account or device,
/// a bad signature) is final.
macro_rules! with_session {
    ($driver:expr, $remote:expr, $who:expr, $call:expr) => {
        match $call.await {
            Err(SyncError::Unauthorized) if $remote.needs_authentication($who.0, $who.1) => {
                tracing::debug!("session refused: authenticating again");
                $driver.authenticate($remote, $who).await?;
                $call.await.map_err(NetError::from)
            }
            other => other.map_err(NetError::from),
        }
    };
}

mod driver;
mod feed;
mod links;
mod remotes;
mod socket;
#[cfg(test)]
mod tests;

pub use driver::SyncDriver;

/// The calls of one v2 sync cycle (see [`kb_sync::v2::RemoteStore`] for their meaning and their
/// errors): a boxed, `Send` mirror of that trait, implemented for `HttpRemote` and for
/// `kb_sync::v2::MemoryRemote` (`remotes.rs`).
pub trait SyncRemote: Send + Sync + 'static {
    /// `server_info`.
    fn server_info(&self) -> RemoteFuture<'_, ServerInfo>;
    /// `create_account` with an operator invite token.
    fn create_account<'a>(
        &'a self,
        invite_token: &'a str,
        log: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, ()>;
    /// `account_create_share`.
    fn account_create_share<'a>(
        &'a self,
        log: &'a [Vec<u8>],
        invite_id: InviteId,
        sig: &'a Signature,
    ) -> RemoteFuture<'a, ()>;
    /// `append_log`.
    fn append_log<'a>(&'a self, account: AccountId, entries: &'a [Vec<u8>])
    -> RemoteFuture<'a, ()>;
    /// `fetch_log`.
    fn fetch_log(&self, account: AccountId, after: Option<u64>) -> RemoteFuture<'_, Vec<Vec<u8>>>;
    /// `logs_batch`.
    fn logs_batch<'a>(&'a self, requests: &'a [LogRequest]) -> RemoteFuture<'a, Vec<LogChunk>>;
    /// Whether [`SyncRemote::authenticate`] is needed before the authenticated calls (a remote that
    /// cannot tell says `true`).
    fn needs_authentication(&self, account: AccountId, device: DeviceId) -> bool {
        let _ = (account, device);
        true
    }
    /// Opens a session, `signer` signing the challenge.
    fn authenticate(
        &self,
        account: AccountId,
        device: DeviceId,
        signer: AuthSigner,
    ) -> RemoteFuture<'_, ()>;
    /// `feeds_create`.
    fn feeds_create<'a>(&'a self, create: &'a [u8]) -> RemoteFuture<'a, FeedCreated>;
    /// `feeds_list`.
    fn feeds_list(&self) -> RemoteFuture<'_, Vec<FeedListing>>;
    /// `membership_append`.
    fn membership_append<'a>(
        &'a self,
        feed: FeedId,
        entries: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, MembershipHead>;
    /// `membership_get`.
    fn membership_get(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> RemoteFuture<'_, MembershipPage>;
    /// `append`.
    fn append<'a>(&'a self, feed: FeedId, entry: &'a [u8]) -> RemoteFuture<'a, (u64, [u8; 32])>;
    /// `changes`.
    fn changes(&self, feed: FeedId, after: u64, limit: u32) -> RemoteFuture<'_, ChangesPage>;
    /// `put_snapshot`.
    fn put_snapshot<'a>(&'a self, feed: FeedId, snapshot: &'a [u8]) -> RemoteFuture<'a, ()>;
    /// `get_snapshot`.
    fn get_snapshot(&self, feed: FeedId, doc: DocId) -> RemoteFuture<'_, Option<Vec<u8>>>;
    /// `snapshots`.
    fn snapshots(&self, feed: FeedId) -> RemoteFuture<'_, SnapshotsPage>;
    /// `snapshot_marks`.
    fn snapshot_marks(&self, feed: FeedId, after: u64) -> RemoteFuture<'_, MarksPage>;
    /// `snapshot_dispute`.
    fn snapshot_dispute(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> RemoteFuture<'_, ()>;
    /// `put_head`.
    fn put_head<'a>(&'a self, feed: FeedId, head: &'a [u8]) -> RemoteFuture<'a, ()>;
    /// `heads`.
    fn heads(&self, feed: FeedId) -> RemoteFuture<'_, Vec<Vec<u8>>>;
    /// `put_chunk`.
    fn put_chunk<'a>(
        &'a self,
        feed: FeedId,
        hash: ChunkId,
        bytes: &'a [u8],
    ) -> RemoteFuture<'a, ()>;
    /// `get_chunk`.
    fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, Option<Vec<u8>>>;
    /// `delete_chunk`.
    fn delete_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, ()>;
    /// `copy_chunk`.
    fn copy_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, ()>;
    /// `invite_open`.
    fn invite_open<'a>(
        &'a self,
        invite_id: InviteId,
        sig: &'a Signature,
    ) -> RemoteFuture<'a, InviteOpened>;
    /// `link_put` (M13 slice 4): uploads a view link's publication; needs the session. The default
    /// is a remote with no view links.
    fn link_put<'a>(&'a self, link_id: LinkId, put: &'a LinkPut) -> RemoteFuture<'a, u64> {
        let _ = (link_id, put);
        Box::pin(std::future::ready(Err(SyncError::Protocol(
            "this remote has no view links".to_owned(),
        ))))
    }
    /// `link_delete` (M13 slice 4): revokes a view link on the server; needs the session. The
    /// default is a remote with no view links.
    fn link_delete(&self, link_id: LinkId) -> RemoteFuture<'_, ()> {
        let _ = link_id;
        Box::pin(std::future::ready(Err(SyncError::Protocol(
            "this remote has no view links".to_owned(),
        ))))
    }
    /// Opens the realtime socket (M13 slice 3, see [`crate::realtime`]): `None` when the remote has none
    /// (the periodic pull still runs, and nothing is relayed). Needs a session (call after
    /// authenticating) and a tokio runtime.
    fn realtime(&self) -> Option<Realtime> {
        None
    }
    /// `pair_create`.
    fn pair_create<'a>(&'a self, offer: &'a [u8]) -> RemoteFuture<'a, ([u8; 6], i64)>;
    /// `pair_messages`.
    fn pair_messages(
        &self,
        id: [u8; 6],
        after: u32,
        wait_seconds: u32,
    ) -> RemoteFuture<'_, PairingPage>;
    /// `pair_post`.
    fn pair_post<'a>(&'a self, id: [u8; 6], role: u8, bytes: &'a [u8]) -> RemoteFuture<'a, ()>;
    /// `pair_close`.
    fn pair_close(&self, id: [u8; 6]) -> RemoteFuture<'_, ()>;
    /// `recovery_put`.
    fn recovery_put<'a>(&'a self, id: [u8; 32], bytes: &'a [u8]) -> RemoteFuture<'a, ()>;
    /// `recovery_get`.
    fn recovery_get(&self, id: [u8; 32]) -> RemoteFuture<'_, Option<Vec<u8>>>;
    /// `recovery_delete`.
    fn recovery_delete(&self, id: [u8; 32]) -> RemoteFuture<'_, ()>;
}

/// What the v2 driver needs from the core (the shell's `AppState` implements it; every method runs
/// under the single-writer lock and answers synchronously, so the driver calls them on tokio's
/// blocking pool). The shapes mirror `kb_core::Core`'s methods one to one; their documentation is
/// there.
pub trait SyncHost: Send + Sync + 'static {
    // ----- the account -----

    /// The sync status, including the feeds.
    fn sync_info(&self) -> Result<SyncInfo, ApiError>;
    /// The account id and this device's id, once the account exists.
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
    /// The invite token given with the last `SyncConfigure`, handed out once.
    fn sync_take_invite_token(&self) -> Result<Option<String>, ApiError>;
    /// The server does not know the account and there is no (usable) invite token.
    fn sync_account_unknown(&self) -> Result<(), ApiError>;
    /// The account's own device log as the server holds it.
    fn sync_log_received(&self, entries: &[Vec<u8>]) -> Result<bool, ApiError>;
    /// This account's device log and how many entries, from the start, are published.
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
    /// The server holds every log entry with a sequence number `<= up_to_seq`.
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError>;
    /// Signs an authentication message with the device signing key.
    fn sync_sign(&self, message: &[u8]) -> Result<kb_crypto::Signature, ApiError>;
    /// First contact or an epoch change; `true` when the core reset its cursor state (every feed).
    fn sync_remote_epoch(&self, epoch: [u8; 16]) -> Result<bool, ApiError>;
    /// Reports where the driver stands.
    fn sync_set_state(&self, state: SyncState, error: Option<String>) -> Result<(), ApiError>;
    /// The recovery blob to upload while the server does not hold the current one.
    fn sync_recovery_blob(&self) -> Result<Option<RecoveryUpload>, ApiError>;
    /// The server took the recovery blob.
    fn sync_recovery_published(&self) -> Result<(), ApiError>;
    /// Lookup ids of recovery blobs that must leave the server.
    fn sync_recovery_obsolete(&self) -> Result<Vec<[u8; 32]>, ApiError>;
    /// The server no longer holds the blob under `id`.
    fn sync_recovery_deleted(&self, id: [u8; 32]) -> Result<(), ApiError>;

    // ----- feeds and co-members -----

    /// The feeds to work this cycle, the personal one first.
    fn sync_feeds(&self) -> Result<Vec<FeedWork>, ApiError>;
    /// The co-members' device logs the core needs.
    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError>;
    /// A co-member's device log as the server holds it.
    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError>;

    // ----- membership (compare-and-swap) -----

    /// The membership entries to post, in order.
    fn sync_membership_pending(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError>;
    /// The server took them; its head.
    fn sync_membership_posted(&self, feed: FeedId, head: MembershipHead) -> Result<(), ApiError>;
    /// The compare-and-swap lost.
    fn sync_membership_conflict(&self, feed: FeedId) -> Result<(), ApiError>;
    /// Whether the core wants the membership log read directly, and from where.
    fn sync_membership_wanted(&self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError>;
    /// A page of the membership log read directly.
    fn sync_membership_fetched(&self, feed: FeedId, page: &MembershipPage) -> Result<(), ApiError>;
    /// The feed is gone for this account (404).
    fn sync_feed_lost(&self, feed: FeedId) -> Result<(), ApiError>;
    /// The server refused this account on the feed (403).
    fn sync_feed_refused(&self, feed: FeedId, message: &str) -> Result<(), ApiError>;
    /// An append was refused with 409 `rekey_needed`: the feed waits for a `Rekey` (a `Leave` or a
    /// revocation), which the next writer's device mints.
    fn sync_feed_rekey_needed(&self, feed: FeedId) -> Result<(), ApiError>;

    // ----- entries, snapshots, heads, chunks of one feed -----

    /// The entries to upload.
    fn sync_feed_outbox(&self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError>;
    /// The server took an entry.
    fn sync_feed_pushed(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        up_to_seq: i64,
    ) -> Result<(), ApiError>;
    /// A page of changes to apply.
    fn sync_feed_apply(&self, feed: FeedId, page: &ChangesPage) -> Result<SyncApplied, ApiError>;
    /// Documents due a new snapshot.
    fn sync_feed_snapshot_candidates(&self, feed: FeedId) -> Result<Vec<DocId>, ApiError>;
    /// Makes a snapshot.
    fn sync_feed_make_snapshot(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(Vec<u8>, u64), ApiError>;
    /// The server took the snapshot.
    fn sync_feed_snapshot_published(
        &self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError>;
    /// Snapshots to apply (and verify).
    fn sync_feed_apply_snapshots(
        &self,
        feed: FeedId,
        page: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError>;
    /// Whether to verify snapshots here, from which mark.
    fn sync_snapshot_marks_wanted(&self, feed: FeedId) -> Result<Option<u64>, ApiError>;
    /// The marks the server announced; the documents to fetch.
    fn sync_snapshot_marks_received(
        &self,
        feed: FeedId,
        marks: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError>;
    /// The check of a document's snapshot is over (`ok` false: a dispute was posted).
    fn sync_snapshot_verified(&self, feed: FeedId, doc_id: DocId, ok: bool)
    -> Result<(), ApiError>;
    /// This device's signed head for the feed.
    fn sync_feed_head(&self, feed: FeedId) -> Result<Vec<u8>, ApiError>;
    /// The other devices' heads; `true` when a fork was detected.
    fn sync_feed_heads_received(&self, feed: FeedId, heads: &[Vec<u8>]) -> Result<bool, ApiError>;
    /// Chunk transfers due.
    fn sync_feed_chunk_work(
        &self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError>;
    /// The sealed chunk to upload.
    fn sync_feed_chunk_bytes(
        &self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError>;
    /// The server took the chunk.
    fn sync_feed_chunk_uploaded(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
    /// A downloaded chunk.
    fn sync_feed_chunk_received(
        &self,
        feed: FeedId,
        hash: ChunkId,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError>;
    /// The server does not hold the chunk yet.
    fn sync_feed_chunk_unavailable(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
    /// The server no longer holds the chunk.
    fn sync_feed_chunk_deleted(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;

    // ----- realtime and presence (M13 slice 3); the default bodies are those of a host with no presence -----

    /// Offers an entry the socket relayed inline to the feed's apply path
    /// (`kb_core::Core::sync_feed_apply_inline`). The default always answers `Pull`: the entry comes
    /// with the next pull of the feed.
    fn sync_feed_apply_inline(
        &self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError> {
        let _ = (feed, inline);
        Ok(InlineOutcome::Pull)
    }
    /// The push debounce to use now, in milliseconds (`kb_core::Core::sync_push_debounce_ms`: 150 while a
    /// remote presence is held, else 500). The default is the plain debounce.
    fn sync_push_debounce_ms(&self) -> Result<u64, ApiError> {
        Ok(PUSH_DEBOUNCE_MS)
    }
    /// A presence frame the socket relayed for `feed`: the host opens it (`kb_core::Core::presence_open`)
    /// and hands an accepted one to the shell's channel. The default drops it.
    fn presence_received(&self, feed: FeedId, envelope: &[u8]) -> Result<(), ApiError> {
        let _ = (feed, envelope);
        Ok(())
    }
    /// Drops timed-out presence (`kb_core::Core::presence_expire`); the driver calls it about every five
    /// seconds while it has a socket. The default holds none.
    fn presence_expire(&self) -> Result<(), ApiError> {
        Ok(())
    }

    // ----- view links (M13 slice 4); the default bodies are those of a host with no view links -----

    /// The view-link uploads and deletions due now (`kb_core::Core::sync_link_work`): the driver runs
    /// them after the chunks of each cycle, through `SyncRemote::link_put` and `link_delete`, and reports
    /// each outcome with [`SyncHost::sync_link_done`]. The default has none.
    fn sync_link_work(&self) -> Result<Vec<LinkWork>, ApiError> {
        Ok(Vec::new())
    }
    /// How the server answered one piece of link work (`kb_core::Core::sync_link_done`). The default
    /// ignores it.
    fn sync_link_done(&self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError> {
        let _ = (link_id, outcome);
        Ok(())
    }

    /// Core events (the driver reacts to `SyncRequested` and `SyncChanged`). The callback may run
    /// under the core's lock: it must not block. The returned closure unsubscribes.
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send>;
}
