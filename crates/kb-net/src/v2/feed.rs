//! The steps of one feed in a cycle of the v2 driver, in the order of `crates/kb-core/src/feeds.rs`
//! (the recipe the core was built against): membership, push, snapshots to publish, pull, the marks of
//! other devices' snapshots, chunks, head. A shared feed's failures that mean "this account is out of
//! the feed" (403, 404, closed) end that step and tell the core; anything else is the caller's.

use std::sync::Arc;
use std::time::Duration;

use kb_sync::v2::{FeedChunkKind, FeedChunkWork, FeedWork, OutboundEntry, peek_membership};
use kb_sync::{ChangesPage, SnapshotsPage, SyncError};
use kb_types::sync::{CHANGES_PAGE_LIMIT, FeedKind, TRANSFER_BUDGET};
use kb_types::{DocId, FeedId};
use tokio::time::Instant;

use super::SyncRemote;
use super::driver::{Cx, Driver, Summary};
use crate::NetError;
use crate::driver::Plan;

/// Pages of the membership log one cycle reads (one unless the page says `more`).
const MAX_MEMBERSHIP_PAGES: usize = 50;
/// Pages of snapshot marks one cycle reads.
const MAX_MARK_PAGES: usize = 20;
/// Pages of changes one pull reads before it leaves the rest for the next cycle.
const MAX_PULL_PAGES: usize = 10_000;
/// How often a feed's snapshots are fetched again because items still wait for them (items that
/// were parked by the pull itself ask at once).
const PARKED_REFRESH: Duration = Duration::from_secs(600);
/// What a refusal of the server on a shared feed is called in the core's diagnostics.
const REFUSED: &str = "forbidden";

/// Whether the feed's step goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    /// The feed is gone for this account: the core was told.
    End,
}

/// Deletes first (freed quota before new uploads), then copies and uploads, then downloads.
const fn transfer_rank(kind: FeedChunkKind) -> u8 {
    match kind {
        FeedChunkKind::Delete => 0,
        FeedChunkKind::Copy | FeedChunkKind::Upload => 1,
        FeedChunkKind::Download => 2,
    }
}

/// What the chunk step of a feed did (counts only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Transferred {
    deleted: usize,
    uploaded: usize,
    copied: usize,
    downloaded: usize,
    /// Downloads the server could not serve yet.
    waiting: usize,
    /// Uploads whose chunk is no longer here.
    skipped: usize,
    /// The server refused an upload for its quota: the other uploads of the cycle wait.
    quota_full: bool,
    /// The feed refuses writes for this account (closed, or its role is too low): only downloads run.
    writes_closed: bool,
}

impl Transferred {
    /// Items carried out (the core was told the outcome of each).
    const fn carried_out(&self) -> usize {
        self.deleted + self.uploaded + self.copied + self.downloaded + self.waiting + self.skipped
    }
}

impl<R: SyncRemote> Driver<R> {
    /// One feed of the cycle: membership (a shared feed), push and snapshots (when the cycle uploads
    /// and the feed is neither forked nor closed), pull, marks (a shared feed), chunks, head.
    pub(super) async fn feed_cycle(
        &mut self,
        remote: &Arc<R>,
        cx: Cx,
        work: &FeedWork,
        summary: &mut Summary,
    ) -> Result<(), NetError> {
        let feed = work.feed;
        let shared = work.kind == FeedKind::Shared;
        if shared && self.membership(remote, cx, feed, summary).await? == Flow::End {
            return Ok(());
        }
        let mut stale = false;
        if cx.push && !work.forked && !work.closed {
            stale = self.push(remote, cx, feed, shared).await?;
            self.snapshot_uploads(remote, cx, feed).await?;
        }
        self.pull(remote, cx, work, summary).await?;
        if stale && !work.forked && !work.closed {
            // The pull taught the feed's new epoch: the outbox is rebuilt and resealed under it.
            self.push(remote, cx, feed, shared).await?;
        }
        if shared {
            self.marks(remote, cx, feed).await?;
        }
        if !work.forked {
            self.chunks(remote, cx, work, summary).await?;
        }
        self.heads(remote, cx, feed, shared).await
    }

    // ----- membership -----------------------------------------------------------------------------

    /// The membership entries the core minted, by compare-and-swap, and the log read directly when the
    /// core wants it.
    async fn membership(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
        summary: &mut Summary,
    ) -> Result<Flow, NetError> {
        let who = cx.who;
        let pending = self
            .host_call(move |host| host.sync_membership_pending(feed))
            .await?;
        if !pending.is_empty() {
            // The `Create` (seq 0) makes the feed; everything else is appended to its log.
            let creates = match pending.first() {
                Some(first) => peek_membership(first).map_err(NetError::from)?.seq == 0,
                None => false,
            };
            let (create, rest) = if creates {
                (
                    pending.first().cloned(),
                    pending.get(1..).unwrap_or_default(),
                )
            } else {
                (None, pending.as_slice())
            };
            let mut head = None;
            let mut lost = false;
            if let Some(create) = create {
                match with_session!(self, remote, who, remote.feeds_create(&create)) {
                    Ok(created) => head = Some(created.head),
                    Err(NetError::Sync(SyncError::Conflict(_))) => lost = true,
                    Err(NetError::Sync(SyncError::Forbidden)) => {
                        self.refused(feed).await?;
                        return Ok(Flow::Continue);
                    }
                    Err(error) => return Err(error),
                }
            }
            if !lost && !rest.is_empty() {
                match with_session!(self, remote, who, remote.membership_append(feed, rest)) {
                    Ok(taken) => head = Some(taken),
                    Err(NetError::Sync(SyncError::Conflict(_))) => lost = true,
                    Err(NetError::Sync(SyncError::NotFound)) => {
                        // The server does not know this account in the feed any more.
                        self.lost(feed).await?;
                        return Ok(Flow::End);
                    }
                    Err(NetError::Sync(SyncError::Forbidden)) => {
                        self.refused(feed).await?;
                        return Ok(Flow::Continue);
                    }
                    Err(NetError::Sync(SyncError::Closed)) => return Ok(Flow::Continue),
                    Err(error) => return Err(error),
                }
            }
            if lost {
                tracing::debug!(feed = %feed.short_hex(), "the membership compare-and-swap was lost");
                self.host_call(move |host| host.sync_membership_conflict(feed))
                    .await?;
            } else if let Some(head) = head {
                self.host_call(move |host| host.sync_membership_posted(feed, head))
                    .await?;
                summary.membership_posted = true;
            }
        }
        for _ in 0..MAX_MEMBERSHIP_PAGES {
            let Some(after) = self
                .host_call(move |host| host.sync_membership_wanted(feed))
                .await?
            else {
                break;
            };
            let page = match with_session!(self, remote, who, remote.membership_get(feed, after)) {
                Ok(page) => page,
                Err(NetError::Sync(SyncError::NotFound | SyncError::Forbidden)) => break,
                Err(error) => return Err(error),
            };
            let more = page.more;
            self.host_call(move |host| host.sync_membership_fetched(feed, &page))
                .await?;
            if !more {
                break;
            }
        }
        Ok(Flow::Continue)
    }

    // ----- push -----------------------------------------------------------------------------------

    /// Uploads the feed's outbox one entry at a time, stopping at the first refusal. `true` when the
    /// feed's epoch moved on (409 `stale_epoch`): the outbox is worth another try after the pull.
    async fn push(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
        shared: bool,
    ) -> Result<bool, NetError> {
        let who = cx.who;
        let outbox = self
            .host_call(move |host| host.sync_feed_outbox(feed))
            .await?;
        let count = outbox.len();
        let mut pushed = 0usize;
        let mut stale = false;
        for entry in outbox {
            let OutboundEntry {
                doc_id,
                device_seq,
                bytes,
                up_to_seq,
            } = entry;
            match with_session!(self, remote, who, remote.append(feed, &bytes)) {
                Ok((cursor, _)) => {
                    self.host_call(move |host| {
                        host.sync_feed_pushed(feed, doc_id, device_seq, cursor, up_to_seq)
                    })
                    .await?;
                    pushed += 1;
                }
                Err(NetError::Sync(SyncError::RekeyNeeded)) => {
                    self.host_call(move |host| host.sync_feed_rekey_needed(feed))
                        .await?;
                    break;
                }
                Err(NetError::Sync(SyncError::StaleEpoch)) => {
                    stale = true;
                    break;
                }
                Err(NetError::Sync(SyncError::Closed | SyncError::Conflict(_))) => break,
                Err(NetError::Sync(SyncError::Forbidden)) if shared => {
                    self.refused(feed).await?;
                    break;
                }
                Err(NetError::Sync(SyncError::NotFound)) if shared => {
                    self.lost(feed).await?;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if count > 0 {
            tracing::debug!(feed = %feed.short_hex(), entries = count, pushed, "pushed");
        }
        Ok(stale)
    }

    /// Publishes the snapshots the core says are due. A snapshot the server keeps a newer one of
    /// (`Conflict`) is skipped; a refusal that concerns the feed ends the step.
    async fn snapshot_uploads(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
    ) -> Result<(), NetError> {
        let who = cx.who;
        let candidates = self
            .host_call(move |host| host.sync_feed_snapshot_candidates(feed))
            .await?;
        let mut published = 0usize;
        for doc_id in candidates {
            let (bytes, covers) = self
                .host_call(move |host| host.sync_feed_make_snapshot(feed, doc_id))
                .await?;
            match with_session!(self, remote, who, remote.put_snapshot(feed, &bytes)) {
                Ok(()) => {
                    self.host_call(move |host| {
                        host.sync_feed_snapshot_published(feed, doc_id, covers)
                    })
                    .await?;
                    published += 1;
                }
                Err(NetError::Sync(SyncError::Conflict(_))) => {
                    tracing::debug!("the server keeps a newer snapshot");
                }
                Err(NetError::Sync(
                    SyncError::Forbidden
                    | SyncError::NotFound
                    | SyncError::RekeyNeeded
                    | SyncError::StaleEpoch
                    | SyncError::Closed,
                )) => break,
                Err(error) => return Err(error),
            }
        }
        if published > 0 {
            tracing::debug!(feed = %feed.short_hex(), snapshots = published, "snapshots published");
        }
        Ok(())
    }

    // ----- pull -----------------------------------------------------------------------------------

    async fn apply_page(
        &self,
        feed: FeedId,
        page: &Arc<ChangesPage>,
    ) -> Result<kb_sync::v2::SyncApplied, NetError> {
        let page = Arc::clone(page);
        self.host_call(move |host| host.sync_feed_apply(feed, &page))
            .await
    }

    /// `changes` pages from the feed's cursor through the core. A page whose cursor stalled for a
    /// co-member's device log is offered once more after the logs were fetched; cursor gaps (the server
    /// pruned entries), a feed that begins at cursor 0 and items that wait for what a snapshot brings
    /// fetch the feed's snapshots.
    async fn pull(
        &mut self,
        remote: &Arc<R>,
        cx: Cx,
        work: &FeedWork,
        summary: &mut Summary,
    ) -> Result<(), NetError> {
        let who = cx.who;
        let feed = work.feed;
        let shared = work.kind == FeedKind::Shared;
        let mut after = work.cursor;
        let started_at_zero = after == 0;
        let mut gaps_seen = false;
        let mut parked_now = false;
        let mut refreshed = false;
        let mut stalled_once = false;
        for _ in 0..MAX_PULL_PAGES {
            let page = match with_session!(
                self,
                remote,
                who,
                remote.changes(feed, after, CHANGES_PAGE_LIMIT)
            ) {
                Ok(page) => page,
                Err(NetError::Sync(SyncError::Forbidden)) if shared => {
                    self.refused(feed).await?;
                    break;
                }
                Err(NetError::Sync(SyncError::NotFound)) if shared => {
                    self.lost(feed).await?;
                    break;
                }
                Err(error) => return Err(error),
            };
            let more = page.more;
            let last = page.changes.last().map(|(cursor, _)| *cursor);
            let page = Arc::new(page);
            let mut applied = self.apply_page(feed, &page).await?;
            if applied.applied + applied.pending + applied.rejected + applied.gaps > 0 {
                tracing::debug!(
                    feed = %feed.short_hex(),
                    applied = applied.applied,
                    skipped_own = applied.skipped_own,
                    pending = applied.pending,
                    rejected = applied.rejected,
                    gaps = applied.gaps,
                    cursor = applied.new_cursor,
                    "pulled"
                );
            }
            parked_now |= applied.pending > 0;
            if applied.gaps > 0 {
                if applied.new_cursor > after {
                    gaps_seen = true;
                } else if refreshed {
                    // The core decides after one refresh of the snapshots.
                    break;
                } else {
                    // Stuck at the gap: the snapshots carry what was pruned; the same page again.
                    refreshed = true;
                    self.refresh_snapshots(remote, cx, feed).await?;
                    continue;
                }
            }
            if let Some(last) = last
                && applied.new_cursor < last
            {
                // Stalled for a co-member's device log: fetch it and offer the page once more.
                if stalled_once {
                    break;
                }
                stalled_once = true;
                self.foreign_logs(remote, summary).await?;
                applied = self.apply_page(feed, &page).await?;
                if applied.new_cursor < last {
                    after = after.max(applied.new_cursor);
                    self.note_cursor(feed, after);
                    break;
                }
            }
            let advanced = applied.new_cursor > after;
            after = after.max(applied.new_cursor);
            self.note_cursor(feed, after);
            if !more {
                break;
            }
            if !advanced {
                return Err(NetError::Sync(SyncError::Protocol(
                    "the server announced more changes but sent none past the cursor".to_owned(),
                )));
            }
        }
        self.after_pull(remote, cx, feed, started_at_zero, gaps_seen || parked_now)
            .await
    }

    /// The cursor this feed's pull reached, for the realtime socket's `changed` frames.
    fn note_cursor(&mut self, feed: FeedId, cursor: u64) {
        if let Some(link) = self.link.as_mut() {
            link.note_cursor(feed, cursor);
        }
    }

    /// A pull of one feed outside a cycle (the realtime socket announced growth, or relayed an entry the
    /// core could not apply from where it is): the feed's membership log when the core wants it, then
    /// its changes, through the same steps as a cycle's pull, with the session and the epoch the last
    /// cycle found. No state is reported and nothing is pushed: the timer's cycle does that. When it
    /// cannot run (no cycle ran yet, the feed is not the core's, a step failed) a pull of everything is
    /// scheduled instead, which reports whatever is wrong.
    pub(super) async fn pull_feed(&mut self, plan: &mut Plan, feed: FeedId) {
        if !plan.active {
            return;
        }
        match self.try_pull_feed(feed).await {
            Ok(true) => {}
            Ok(false) | Err(_) => plan.pull_by(Instant::now()),
        }
    }

    async fn try_pull_feed(&mut self, feed: FeedId) -> Result<bool, NetError> {
        let Some(link) = self.link.as_ref() else {
            return Ok(false);
        };
        let Some((who, epoch)) = link.session else {
            return Ok(false);
        };
        let remote = Arc::clone(&link.remote);
        let feeds = self.host_call(|host| host.sync_feeds()).await?;
        let Some(work) = feeds.into_iter().find(|work| work.feed == feed) else {
            return Ok(false);
        };
        let cx = Cx {
            who,
            epoch,
            push: false,
        };
        let mut summary = Summary::default();
        if work.kind == FeedKind::Shared
            && self.membership(&remote, cx, feed, &mut summary).await? == Flow::End
        {
            return Ok(true);
        }
        self.pull(&remote, cx, &work, &mut summary).await?;
        tracing::debug!(feed = %feed.short_hex(), "pulled one feed");
        Ok(true)
    }

    /// Fetches the feed's snapshots when the pull asked for them (`wanted`), when it began at cursor 0
    /// (once per connection), or when items still wait for what a snapshot brings (every
    /// [`PARKED_REFRESH`]).
    async fn after_pull(
        &mut self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
        started_at_zero: bool,
        wanted: bool,
    ) -> Result<(), NetError> {
        let bootstrapped = self
            .link
            .as_ref()
            .is_some_and(|link| link.bootstrapped.contains(&feed));
        let mut refresh = wanted || (started_at_zero && !bootstrapped);
        if !refresh {
            let parked = self.host_call(|host| host.sync_info()).await?.parked_items;
            let due = self.link.as_ref().is_none_or(|link| {
                link.parked
                    .get(&feed)
                    .is_none_or(|at| at.elapsed() >= PARKED_REFRESH)
            });
            refresh = parked > 0 && due;
        }
        if refresh {
            self.refresh_snapshots(remote, cx, feed).await?;
        }
        if started_at_zero && let Some(link) = self.link.as_mut() {
            link.bootstrapped.insert(feed);
        }
        Ok(())
    }

    /// The feed's latest snapshots, through the core; each one the core found to omit entries it covers
    /// is disputed.
    async fn refresh_snapshots(
        &mut self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
    ) -> Result<(), NetError> {
        let who = cx.who;
        if let Some(link) = self.link.as_mut() {
            link.parked.insert(feed, Instant::now());
        }
        let page = match with_session!(self, remote, who, remote.snapshots(feed)) {
            Ok(page) => page,
            Err(NetError::Sync(SyncError::NotFound | SyncError::Forbidden)) => return Ok(()),
            Err(error) => return Err(error),
        };
        let applied = self
            .host_call(move |host| host.sync_feed_apply_snapshots(feed, &page))
            .await?;
        if applied.applied > 0 {
            tracing::debug!(feed = %feed.short_hex(), merged = applied.applied, "snapshots applied");
        }
        self.dispute(remote, cx, feed, applied.disputed).await
    }

    /// Posts a dispute for each snapshot the core refused, and tells the core (it republishes).
    async fn dispute(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
        disputed: Vec<(DocId, u64)>,
    ) -> Result<(), NetError> {
        let who = cx.who;
        for (doc, covers) in disputed {
            match with_session!(
                self,
                remote,
                who,
                remote.snapshot_dispute(feed, doc, covers)
            ) {
                // Nothing to dispute any more, or this account may not: the core still republishes.
                Ok(())
                | Err(NetError::Sync(
                    SyncError::NotFound | SyncError::Forbidden | SyncError::Conflict(_),
                )) => {}
                Err(error) => return Err(error),
            }
            tracing::info!(feed = %feed.short_hex(), "a snapshot that omits entries was disputed");
            self.host_call(move |host| host.sync_snapshot_verified(feed, doc, false))
                .await?;
        }
        Ok(())
    }

    // ----- marks ----------------------------------------------------------------------------------

    /// The snapshots other devices published: the ones this device holds documents of are fetched and
    /// verified (snapshot dominance) before the head acknowledges past them.
    async fn marks(&self, remote: &Arc<R>, cx: Cx, feed: FeedId) -> Result<(), NetError> {
        let who = cx.who;
        for _ in 0..MAX_MARK_PAGES {
            let Some(after) = self
                .host_call(move |host| host.sync_snapshot_marks_wanted(feed))
                .await?
            else {
                break;
            };
            let page = match with_session!(self, remote, who, remote.snapshot_marks(feed, after)) {
                Ok(page) => page,
                Err(NetError::Sync(SyncError::NotFound | SyncError::Forbidden)) => break,
                Err(error) => return Err(error),
            };
            let more = page.more;
            let marks = page.marks;
            let docs = self
                .host_call(move |host| host.sync_snapshot_marks_received(feed, &marks))
                .await?;
            for doc in docs {
                self.verify_snapshot(remote, cx, feed, doc).await?;
            }
            if !more {
                break;
            }
        }
        Ok(())
    }

    /// One document's latest snapshot: fetched, checked by the core, disputed when it omits entries.
    async fn verify_snapshot(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
        doc: DocId,
    ) -> Result<(), NetError> {
        let who = cx.who;
        let bytes = match with_session!(self, remote, who, remote.get_snapshot(feed, doc)) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(NetError::Sync(SyncError::NotFound | SyncError::Forbidden)) => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let page = SnapshotsPage {
            server_epoch: cx.epoch,
            snapshots: vec![bytes],
        };
        let applied = self
            .host_call(move |host| host.sync_feed_apply_snapshots(feed, &page))
            .await?;
        if applied.disputed.is_empty() {
            self.host_call(move |host| host.sync_snapshot_verified(feed, doc, true))
                .await
        } else {
            self.dispute(remote, cx, feed, applied.disputed).await
        }
    }

    // ----- chunks ---------------------------------------------------------------------------------

    /// Attachment chunks of the feed (ADR-0023): at most [`TRANSFER_BUDGET`] of them, one at a time (one
    /// chunk in memory), deletes first, then copies and uploads, then downloads. A `Quota` refusal of a
    /// put leaves the remaining uploads for a later cycle; a shared feed that refuses writes leaves them
    /// too (the downloads still run); any other failure ends the step (and the cycle), what was done
    /// before it already recorded by the core.
    async fn chunks(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        work: &FeedWork,
        summary: &mut Summary,
    ) -> Result<(), NetError> {
        let feed = work.feed;
        let shared = work.kind == FeedKind::Shared;
        let mut items = self
            .host_call(move |host| host.sync_feed_chunk_work(feed, TRANSFER_BUDGET))
            .await?;
        // The core hands them out in this order; keep it whatever it sent (a stable sort).
        items.sort_by_key(|item| transfer_rank(item.kind));
        items.truncate(TRANSFER_BUDGET);
        let handed_out = items.len();
        let mut done = Transferred {
            writes_closed: work.closed,
            ..Transferred::default()
        };
        for item in items {
            let write = item.kind != FeedChunkKind::Download;
            if write
                && (done.writes_closed || (done.quota_full && item.kind != FeedChunkKind::Delete))
            {
                continue;
            }
            let result = match item.kind {
                FeedChunkKind::Delete => self.delete_chunk(remote, cx, &item, &mut done).await,
                FeedChunkKind::Upload => self.upload_chunk(remote, cx, &item, &mut done).await,
                FeedChunkKind::Copy => self.copy_chunk(remote, cx, &item, &mut done).await,
                FeedChunkKind::Download => self.download_chunk(remote, cx, &item, &mut done).await,
            };
            match result {
                Ok(()) => {}
                // This account is out of the feed, or may not write to it: the writes wait.
                Err(NetError::Sync(
                    SyncError::Forbidden | SyncError::Closed | SyncError::NotFound,
                )) if shared => {
                    if write {
                        done.writes_closed = true;
                    } else {
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!(
                        feed = %feed.short_hex(),
                        kind = ?item.kind,
                        done = done.carried_out(),
                        "chunk transfer stopped"
                    );
                    return Err(error);
                }
            }
        }
        if handed_out >= TRANSFER_BUDGET && done.carried_out() >= TRANSFER_BUDGET {
            summary.budget_used = true;
        }
        summary.quota_full |= done.quota_full;
        if handed_out > 0 {
            tracing::debug!(
                feed = %feed.short_hex(),
                deleted = done.deleted,
                uploaded = done.uploaded,
                copied = done.copied,
                downloaded = done.downloaded,
                waiting = done.waiting,
                skipped = done.skipped,
                quota_full = done.quota_full,
                "chunks transferred"
            );
        }
        Ok(())
    }

    /// `delete_chunk` (a chunk the server does not hold counts as deleted), then `sync_feed_chunk_deleted`.
    async fn delete_chunk(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        item: &FeedChunkWork,
        done: &mut Transferred,
    ) -> Result<(), NetError> {
        let (feed, hash, who) = (item.feed, item.hash, cx.who);
        with_session!(self, remote, who, remote.delete_chunk(feed, hash))?;
        self.host_call(move |host| host.sync_feed_chunk_deleted(feed, hash))
            .await?;
        done.deleted += 1;
        Ok(())
    }

    /// `sync_feed_chunk_bytes`, `put_chunk`, `sync_feed_chunk_uploaded`. A chunk that is gone here is
    /// skipped; a `Quota` refusal is recorded in `done` (the caller stops the uploads).
    async fn upload_chunk(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        item: &FeedChunkWork,
        done: &mut Transferred,
    ) -> Result<(), NetError> {
        let (feed, hash, who) = (item.feed, item.hash, cx.who);
        let Some(bytes) = self
            .host_call(move |host| host.sync_feed_chunk_bytes(feed, hash))
            .await?
        else {
            done.skipped += 1;
            return Ok(());
        };
        let put = with_session!(self, remote, who, remote.put_chunk(feed, hash, &bytes));
        // One chunk in memory at a time.
        drop(bytes);
        match put {
            Ok(()) => {}
            Err(NetError::Sync(SyncError::Quota)) => {
                tracing::info!("the server's storage quota is full: uploads wait");
                done.quota_full = true;
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        self.host_call(move |host| host.sync_feed_chunk_uploaded(feed, hash))
            .await?;
        done.uploaded += 1;
        Ok(())
    }

    /// `copy_chunk` from the account's personal namespace (the share step), then
    /// `sync_feed_chunk_uploaded`; when the personal namespace does not hold the chunk, it is uploaded
    /// from here.
    async fn copy_chunk(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        item: &FeedChunkWork,
        done: &mut Transferred,
    ) -> Result<(), NetError> {
        let (feed, hash, who) = (item.feed, item.hash, cx.who);
        match with_session!(self, remote, who, remote.copy_chunk(feed, hash)) {
            Ok(()) => {
                self.host_call(move |host| host.sync_feed_chunk_uploaded(feed, hash))
                    .await?;
                done.copied += 1;
                Ok(())
            }
            Err(NetError::Sync(SyncError::NotFound)) => {
                self.upload_chunk(remote, cx, item, done).await
            }
            Err(error) => Err(error),
        }
    }

    /// `get_chunk`, then `sync_feed_chunk_received` with the bytes, or `sync_feed_chunk_unavailable`
    /// when the server does not hold the chunk yet.
    async fn download_chunk(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        item: &FeedChunkWork,
        done: &mut Transferred,
    ) -> Result<(), NetError> {
        let (feed, hash, who) = (item.feed, item.hash, cx.who);
        match with_session!(self, remote, who, remote.get_chunk(feed, hash))? {
            Some(bytes) => {
                self.host_call(move |host| host.sync_feed_chunk_received(feed, hash, bytes))
                    .await?;
                done.downloaded += 1;
            }
            None => {
                self.host_call(move |host| host.sync_feed_chunk_unavailable(feed, hash))
                    .await?;
                done.waiting += 1;
            }
        }
        Ok(())
    }

    // ----- heads ----------------------------------------------------------------------------------

    /// Publishes this device's head of the feed and checks everyone's (a fork is the core's to flag).
    async fn heads(
        &self,
        remote: &Arc<R>,
        cx: Cx,
        feed: FeedId,
        shared: bool,
    ) -> Result<(), NetError> {
        let who = cx.who;
        let head = self
            .host_call(move |host| host.sync_feed_head(feed))
            .await?;
        match with_session!(self, remote, who, remote.put_head(feed, &head)) {
            Ok(()) => {}
            // The feed is closed, or this account is out of it: nothing to acknowledge.
            Err(NetError::Sync(SyncError::NotFound | SyncError::Forbidden | SyncError::Closed))
                if shared =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        let heads = match with_session!(self, remote, who, remote.heads(feed)) {
            Ok(heads) => heads,
            Err(NetError::Sync(SyncError::NotFound | SyncError::Forbidden)) if shared => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if self
            .host_call(move |host| host.sync_feed_heads_received(feed, &heads))
            .await?
        {
            tracing::debug!(feed = %feed.short_hex(), "a fork was detected in the heads");
        }
        Ok(())
    }

    // ----- what the core is told ------------------------------------------------------------------

    /// The server refused this account on the feed (403).
    async fn refused(&self, feed: FeedId) -> Result<(), NetError> {
        self.host_call(move |host| host.sync_feed_refused(feed, REFUSED))
            .await
    }

    /// The server does not know the feed for this account (404).
    async fn lost(&self, feed: FeedId) -> Result<(), NetError> {
        self.host_call(move |host| host.sync_feed_lost(feed)).await
    }
}
