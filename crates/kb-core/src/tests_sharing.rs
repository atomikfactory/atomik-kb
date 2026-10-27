//! Milestone 13, slice 1: the multi-account simulator. People (an account with one or more devices)
//! share one `MemoryRemote` (protocol v2); a scripted cycle runs the way `kb-net`'s v2 driver does
//! (personal feed, the co-members' logs, each shared feed: membership, push, snapshots, pull,
//! marks, chunks, heads), and the scenarios check convergence, the roles, quarantine, the Rekey
//! after a `Leave`, a stale epoch, snapshot dominance, fork-out and the revocation of a device.
//!
//! Joining is simulated at the level the request side will build on (`Core::membership_mint` for the
//! `Invite` and the `Join`, `invite_open` of the remote, the space key handed over like a bundle
//! would): the invite sessions themselves belong to `invites.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines
)]

use std::sync::Arc;

use kb_keys::LogSigner;
use kb_keys::invite::{InviteSecret, invite_id, invite_signing_key, sign_join_proof, sign_open};
use kb_keys::membership::{ChainHead, EntryBody, MembershipEntry, Private};
use kb_storage::{ChunkSyncQueries, DeviceLogRow, FeedQueries, SecurityQueries};
use kb_sync::SnapshotsPage;
use kb_sync::v2::memory::{Adversary, DEFAULT_INVITE_TOKEN};
use kb_sync::v2::{MembershipItem, MembershipPage, MemoryRemote, SyncApplied};
use kb_types::sharing::SpaceSettings;
use kb_types::sync::FeedKind;
use kb_types::testing::{FixedClock, SeededRng};
use kb_types::{
    ApiError, ArchiveReason, Clock, DocId, ErrorCode, Event, FeedId, MemberRole, PageId, SpaceId,
    SyncState,
};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, export_backup, import_backup, kit_create,
    outline, page_content, tree, write_text,
};

mod comments;
mod links;
mod presence;
mod requests;
mod rules;
mod scenarios;
mod template_create;

pub(crate) const SERVER_URL: &str = "https://sync.example.test";
const DAY_MS: i64 = 86_400_000;
const SERVER_NOW: i64 = 1_000_000;

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum SimError {
    Api(ApiError),
    Sync(kb_sync::SyncError),
}

impl From<ApiError> for SimError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<kb_sync::SyncError> for SimError {
    fn from(e: kb_sync::SyncError) -> Self {
        Self::Sync(e)
    }
}

pub(crate) type Sim<T = ()> = Result<T, SimError>;

pub(crate) fn server() -> MemoryRemote {
    let remote = MemoryRemote::new(&SeededRng::new(99));
    remote.set_now_ms(SERVER_NOW);
    remote
}

/// One simulated device: its own installation, keystore, clock and connection.
pub(crate) struct Dev {
    pub(crate) name: &'static str,
    pub(crate) _fx: Fixture,
    pub(crate) core: Core,
    pub(crate) remote: MemoryRemote,
    pub(crate) authed: bool,
    pub(crate) edits: u64,
    /// What the last pull of each feed reported (the tests look at the quarantine counts).
    pub(crate) last_report: Option<SyncApplied>,
    /// The events the core emitted since the test last asked.
    pub(crate) log: Vec<Event>,
    /// The device's clock: tests move it.
    pub(crate) clock: Arc<FixedClock>,
}

/// A clock several holders move.
struct SharedClock(Arc<FixedClock>);

impl Clock for SharedClock {
    fn now_ms(&self) -> i64 {
        self.0.now_ms()
    }
}

/// Opens a core on a fixture with a clock the test keeps hold of.
pub(crate) fn open_core(fx: &Fixture, seed: u64, now_ms: i64) -> (Core, Arc<FixedClock>) {
    let clock = Arc::new(FixedClock::new(now_ms));
    let mut deps = fx.deps(seed, now_ms);
    deps.clock = Box::new(SharedClock(Arc::clone(&clock)));
    (Core::open(deps).unwrap(), clock)
}

impl Dev {
    /// The first device of an account: workspace, first space, the recovery kit.
    pub(crate) fn first(name: &'static str, seed: u64, server: &MemoryRemote) -> (Self, String) {
        let fx = Fixture::new();
        let (mut core, clock) = open_core(&fx, seed, 1_000_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let kit = kit_create(&mut core);
        let words = kit.words.join(" ");
        core.drain_events();
        (
            Self {
                name,
                _fx: fx,
                core,
                remote: server.clone(),
                authed: false,
                edits: seed * 1000,
                last_report: None,
                log: Vec::new(),
                clock,
            },
            words,
        )
    }

    /// A device that joins the account the way Milestone 4 restores: a backup of `source` plus the
    /// kit on a fresh installation.
    pub(crate) fn from_backup(
        name: &'static str,
        seed: u64,
        server: &MemoryRemote,
        source: &mut Dev,
        kit: &str,
    ) -> Self {
        let bytes = export_backup(&mut source.core);
        let fx = Fixture::new();
        let (mut core, clock) = open_core(&fx, seed, 1_000_000 + seed as i64);
        import_backup(&mut core, &bytes, Some(kit));
        core.drain_events();
        Self {
            name,
            _fx: fx,
            core,
            remote: server.clone(),
            authed: false,
            edits: seed * 1000,
            last_report: None,
            log: Vec::new(),
            clock,
        }
    }

    /// A device that joins with nothing but the ARK and a `DeviceAdded` entry that `sponsor` signs
    /// and publishes (the shape pairing takes): no documents, everything comes from the server.
    pub(crate) fn fresh(
        name: &'static str,
        seed: u64,
        server: &MemoryRemote,
        sponsor: &mut Dev,
    ) -> Self {
        let fx = Fixture::new();
        let ark = sponsor.core.ark.clone();
        let clock = Arc::new(FixedClock::new(1_000_000 + seed as i64));
        let mut deps = fx.deps(seed, 1_000_000 + seed as i64);
        deps.clock = Box::new(SharedClock(Arc::clone(&clock)));
        let mut core = match Core::open_with_ark(deps, ark, None, None) {
            Ok(core) => core,
            Err((_, err)) => panic!("open with the account key: {err}"),
        };
        create_workspace(&mut core, "Home", None);
        let now = sponsor.core.clock.now_ms();
        let encrypted_name = kb_keys::log::encrypt_name(
            &sponsor.core.ark,
            core.device.device_id,
            &core.device.name,
            sponsor.core.rng.as_ref(),
        );
        let entry = sponsor
            .core
            .security
            .log
            .as_mut()
            .unwrap()
            .add_device(
                core.device.device_id,
                core.security.device_keys.public(),
                encrypted_name,
                LogSigner::Device(
                    &sponsor.core.security.device_keys,
                    sponsor.core.device.device_id,
                ),
                now,
            )
            .unwrap()
            .clone();
        sponsor
            .core
            .db
            .device_log_append(&DeviceLogRow {
                seq: entry.seq,
                entry: entry.encode(),
                hash: entry.hash(),
            })
            .unwrap();
        sponsor.core.security.log_state = Some(
            sponsor
                .core
                .security
                .log
                .as_ref()
                .unwrap()
                .verify()
                .unwrap(),
        );
        sponsor.cycle().unwrap();
        let account = sponsor.core.sync_identity().unwrap().unwrap().0;
        let log = server.fetch_log_sync(account, None).unwrap();
        assert!(core.sync_log_received(&log).unwrap());
        core.drain_events();
        Self {
            name,
            _fx: fx,
            core,
            remote: server.clone(),
            authed: false,
            edits: seed * 1000,
            last_report: None,
            log: Vec::new(),
            clock,
        }
    }

    pub(crate) fn account(&self) -> kb_types::AccountId {
        self.core.sync_identity().unwrap().unwrap().0
    }

    pub(crate) fn configure(&mut self) {
        self.core.sync_configure(Some(SERVER_URL), None).unwrap();
        self.core.drain_events();
    }

    /// Closes the core and opens the installation again at `now_ms` (a restart: the state tables,
    /// not memory, carry what was under way).
    pub(crate) fn reopen(&mut self, seed: u64, now_ms: i64) {
        let clock = Arc::new(FixedClock::new(now_ms));
        let mut deps = self._fx.deps(seed, now_ms);
        deps.clock = Box::new(SharedClock(Arc::clone(&clock)));
        let core = match Core::open_or_locked(deps).unwrap() {
            crate::Opened::Ready(core) => core,
            crate::Opened::Locked(_) => panic!("no lock in the simulator"),
        };
        self.core = core;
        self.clock = clock;
        self.core.drain_events();
        self.authed = false;
    }

    // ----- the driver's cycle ---------------------------------------------------------------

    /// One cycle in the driver's order (see the module documentation of `feeds.rs`).
    pub(crate) fn cycle(&mut self) -> Sim {
        let info = self.remote.server_info_sync()?;
        if self.core.sync_remote_epoch(info.server_epoch)? {
            self.authed = false;
        }
        let (account, device) = self
            .core
            .sync_identity()?
            .expect("a kit exists on every simulated device");
        match self.remote.fetch_log_sync(account, None) {
            Ok(entries) => {
                self.core.sync_log_received(&entries)?;
            }
            Err(kb_sync::SyncError::NotFound) => {
                let (entries, _) = self.core.sync_account_log()?;
                let token = self
                    .core
                    .sync_take_invite_token()?
                    .unwrap_or_else(|| DEFAULT_INVITE_TOKEN.to_owned());
                self.remote.create_account_sync(&token, &entries)?;
                self.core.sync_log_published(entries.len() as u64)?;
            }
            Err(err) => return Err(err.into()),
        }
        let (entries, published) = self.core.sync_account_log()?;
        if (published as usize) < entries.len() {
            self.remote
                .append_log_sync(account, &entries[published as usize..])?;
            self.core.sync_log_published(entries.len() as u64)?;
        }
        if !self.authed {
            self.remote.authenticate_sync(account, device, &|message| {
                self.core.sync_sign(message).unwrap()
            })?;
            self.authed = true;
        }
        // The recovery blob reaches the server once per kit; earlier kits leave it.
        if let Some((id, bytes)) = self.core.sync_recovery_blob()? {
            self.remote.recovery_put_sync(id, &bytes)?;
            self.core.sync_recovery_published()?;
        }
        for id in self.core.sync_recovery_obsolete()? {
            match self.remote.recovery_delete_sync(id) {
                Ok(()) | Err(kb_sync::SyncError::NotFound) => {
                    self.core.sync_recovery_deleted(id)?;
                }
                Err(err) => return Err(err.into()),
            }
        }
        let feeds = self.core.sync_feeds()?;
        for work in &feeds {
            if work.kind == FeedKind::Personal {
                self.feed_cycle(work.feed, work.kind, work.forked)?;
            }
        }
        self.fetch_logs()?;
        for work in &feeds {
            if work.kind == FeedKind::Shared {
                self.feed_cycle(work.feed, work.kind, work.forked)?;
            }
        }
        self.link_step()?;
        let state = if self.core.sync_info()?.fork_detected {
            SyncState::Forked
        } else {
            SyncState::Idle
        };
        self.core.sync_set_state(state, None)?;
        let drained = self.core.drain_events();
        self.log.extend(drained);
        Ok(())
    }

    /// The view-link step of the driver's cycle (`kb-net`'s `links.rs`): the uploads and deletions the core
    /// has due, each answered the way the driver maps the server's answers.
    pub(crate) fn link_step(&mut self) -> Sim {
        use kb_sync::SyncError;
        use kb_sync::v2::{LinkOutcome, LinkWork};
        for work in self.core.sync_link_work()? {
            match work {
                LinkWork::Put { link_id, put } => {
                    let outcome = match self.remote.link_put_sync(link_id, &put) {
                        Ok(pub_seq) => LinkOutcome::Published { pub_seq },
                        Err(
                            SyncError::Forbidden
                            | SyncError::NotFound
                            | SyncError::Closed
                            | SyncError::Conflict(_)
                            | SyncError::Quota,
                        ) => LinkOutcome::Refused,
                        Err(err) => return Err(err.into()),
                    };
                    self.core.sync_link_done(link_id, outcome)?;
                }
                LinkWork::Delete { link_id } => {
                    let outcome = match self.remote.link_delete_sync(link_id) {
                        Ok(()) | Err(SyncError::NotFound) => LinkOutcome::Deleted,
                        Err(SyncError::Forbidden) => LinkOutcome::Refused,
                        Err(err) => return Err(err.into()),
                    };
                    self.core.sync_link_done(link_id, outcome)?;
                }
            }
        }
        Ok(())
    }

    /// Several cycles, so that what one device does reaches the others and comes back.
    pub(crate) fn settle(devs: &mut [&mut Dev], rounds: usize) {
        for _ in 0..rounds {
            for dev in devs.iter_mut() {
                dev.cycle()
                    .unwrap_or_else(|e| panic!("{}: cycle failed: {e:?}", dev.name));
            }
        }
    }

    pub(crate) fn fetch_logs(&mut self) -> Sim {
        let requests = self.core.sync_needed_logs()?;
        if requests.is_empty() {
            return Ok(());
        }
        for chunk in self.remote.logs_batch_sync(&requests)? {
            self.core
                .sync_foreign_log_received(chunk.account, &chunk.entries)?;
        }
        Ok(())
    }

    pub(crate) fn feed_cycle(&mut self, feed: FeedId, kind: FeedKind, forked: bool) -> Sim {
        let shared = kind == FeedKind::Shared;
        if shared {
            self.membership_step(feed)?;
        }
        if !forked {
            self.push_step(feed, shared)?;
            self.snapshot_upload_step(feed)?;
        }
        self.pull_step(feed, shared)?;
        if shared {
            self.marks_step(feed)?;
        }
        self.chunk_step(feed)?;
        let head = self.core.sync_feed_head(feed)?;
        match self.remote.put_head_sync(feed, &head) {
            Ok(()) => {
                let heads = self.remote.heads_sync(feed)?;
                self.core.sync_feed_heads_received(feed, &heads)?;
            }
            Err(
                kb_sync::SyncError::NotFound
                | kb_sync::SyncError::Forbidden
                | kb_sync::SyncError::Closed,
            ) if shared => {}
            Err(err) => return Err(err.into()),
        }
        Ok(())
    }

    pub(crate) fn membership_step(&mut self, feed: FeedId) -> Sim {
        let pending = self.core.sync_membership_pending(feed)?;
        if !pending.is_empty() {
            let (create, rest): (Vec<&Vec<u8>>, Vec<&Vec<u8>>) =
                pending.iter().partition(|bytes| {
                    MembershipEntry::peek(bytes).is_ok_and(|header| header.seq == 0)
                });
            let mut head = None;
            let mut lost = false;
            if let Some(create) = create.first() {
                match self.remote.feeds_create_sync(create) {
                    Ok(created) => head = Some(created.head),
                    Err(kb_sync::SyncError::Conflict(_)) => lost = true,
                    Err(err) => return Err(err.into()),
                }
            }
            if !lost && !rest.is_empty() {
                let entries: Vec<Vec<u8>> = rest.into_iter().cloned().collect();
                match self.remote.membership_append_sync(feed, &entries) {
                    Ok(h) => head = Some(h),
                    Err(kb_sync::SyncError::Conflict(_)) => lost = true,
                    Err(kb_sync::SyncError::NotFound) => {
                        // The server does not know this account in the feed any more.
                        self.core.sync_feed_lost(feed)?;
                        return Ok(());
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            if lost {
                self.core.sync_membership_conflict(feed)?;
            } else if let Some(head) = head {
                self.core.sync_membership_posted(feed, head)?;
            }
        }
        for _ in 0..50 {
            let Some(after) = self.core.sync_membership_wanted(feed)? else {
                break;
            };
            let page = match self.remote.membership_get_sync(feed, after) {
                Ok(page) => page,
                Err(kb_sync::SyncError::NotFound | kb_sync::SyncError::Forbidden) => break,
                Err(err) => return Err(err.into()),
            };
            self.core.sync_membership_fetched(feed, &page)?;
            if !page.more {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn push_step(&mut self, feed: FeedId, shared: bool) -> Sim {
        for entry in self.core.sync_feed_outbox(feed)? {
            match self.remote.append_sync(feed, &entry.bytes) {
                Ok((cursor, _)) => {
                    self.core.sync_feed_pushed(
                        feed,
                        entry.doc_id,
                        entry.device_seq,
                        cursor,
                        entry.up_to_seq,
                    )?;
                }
                Err(kb_sync::SyncError::RekeyNeeded) => {
                    self.core.sync_feed_rekey_needed(feed)?;
                    break;
                }
                Err(
                    kb_sync::SyncError::StaleEpoch
                    | kb_sync::SyncError::Closed
                    | kb_sync::SyncError::Conflict(_),
                ) => break,
                Err(kb_sync::SyncError::Forbidden) if shared => {
                    self.core.sync_feed_refused(feed, "forbidden")?;
                    break;
                }
                Err(kb_sync::SyncError::NotFound) if shared => {
                    break;
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    pub(crate) fn snapshot_upload_step(&mut self, feed: FeedId) -> Sim {
        for doc in self.core.sync_feed_snapshot_candidates(feed)? {
            let (bytes, covers) = self.core.sync_feed_make_snapshot(feed, doc)?;
            match self.remote.put_snapshot_sync(feed, &bytes) {
                Ok(()) => self.core.sync_feed_snapshot_published(feed, doc, covers)?,
                Err(
                    kb_sync::SyncError::Conflict(_)
                    | kb_sync::SyncError::Forbidden
                    | kb_sync::SyncError::NotFound
                    | kb_sync::SyncError::RekeyNeeded
                    | kb_sync::SyncError::Closed,
                ) => break,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    pub(crate) fn refresh_snapshots(&mut self, feed: FeedId) -> Sim {
        let page = match self.remote.snapshots_sync(feed) {
            Ok(page) => page,
            Err(kb_sync::SyncError::NotFound | kb_sync::SyncError::Forbidden) => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        let applied = self.core.sync_feed_apply_snapshots(feed, &page)?;
        for (doc, covers) in applied.disputed {
            self.remote.snapshot_dispute_sync(feed, doc, covers)?;
            self.core.sync_snapshot_verified(feed, doc, false)?;
        }
        Ok(())
    }

    pub(crate) fn pull_step(&mut self, feed: FeedId, shared: bool) -> Sim {
        let started_at_zero = self.core.feed_row(feed)?.cursor == 0;
        let mut refreshed = false;
        let mut stalled_once = false;
        for _ in 0..10_000 {
            let cursor = self.core.feed_row(feed)?.cursor;
            let page = match self.remote.changes_sync(feed, cursor, 200) {
                Ok(page) => page,
                Err(kb_sync::SyncError::Forbidden) if shared => {
                    self.core.sync_feed_refused(feed, "forbidden")?;
                    break;
                }
                Err(kb_sync::SyncError::NotFound) if shared => {
                    self.core.sync_feed_lost(feed)?;
                    break;
                }
                Err(err) => return Err(err.into()),
            };
            let more = page.more;
            let last = page.changes.last().map(|(c, _)| *c);
            let report = self.core.sync_feed_apply(feed, &page)?;
            self.last_report = Some(report);
            if report.gaps > 0 && report.new_cursor == cursor {
                assert!(!refreshed, "the core decides after one snapshot refresh");
                refreshed = true;
                self.refresh_snapshots(feed)?;
                continue;
            }
            if report.gaps > 0 {
                self.refresh_snapshots(feed)?;
            }
            if let Some(last) = last
                && report.new_cursor < last
            {
                // Stalled for a co-member's device log: fetch it and offer the page once more.
                if stalled_once {
                    break;
                }
                stalled_once = true;
                self.fetch_logs()?;
                continue;
            }
            if !more {
                break;
            }
        }
        // A feed whose pull started at the beginning brings its snapshots in (parked until the
        // membership they need is known).
        if started_at_zero && shared || self.core.sync_info()?.parked_items > 0 {
            self.refresh_snapshots(feed)?;
        }
        Ok(())
    }

    pub(crate) fn marks_step(&mut self, feed: FeedId) -> Sim {
        let Some(after) = self.core.sync_snapshot_marks_wanted(feed)? else {
            return Ok(());
        };
        let page = match self.remote.snapshot_marks_sync(feed, after) {
            Ok(page) => page,
            Err(kb_sync::SyncError::NotFound | kb_sync::SyncError::Forbidden) => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        let docs = self.core.sync_snapshot_marks_received(feed, &page.marks)?;
        for doc in docs {
            let Some(bytes) = self.remote.get_snapshot_sync(feed, doc)? else {
                continue;
            };
            let epoch = self.remote.server_epoch();
            let applied = self.core.sync_feed_apply_snapshots(
                feed,
                &SnapshotsPage {
                    server_epoch: epoch,
                    snapshots: vec![bytes],
                },
            )?;
            for (doc, covers) in &applied.disputed {
                self.remote.snapshot_dispute_sync(feed, *doc, *covers)?;
                self.core.sync_snapshot_verified(feed, *doc, false)?;
            }
            if applied.disputed.is_empty() {
                self.core.sync_snapshot_verified(feed, doc, true)?;
            }
        }
        Ok(())
    }

    pub(crate) fn chunk_step(&mut self, feed: FeedId) -> Sim {
        use kb_sync::v2::FeedChunkKind;
        for _ in 0..1000 {
            let work = self.core.sync_feed_chunk_work(feed, 8)?;
            if work.is_empty() {
                break;
            }
            let count = work.len();
            for item in work {
                match item.kind {
                    FeedChunkKind::Delete => {
                        self.remote.delete_chunk_sync(feed, item.hash)?;
                        self.core.sync_feed_chunk_deleted(feed, item.hash)?;
                    }
                    FeedChunkKind::Upload => {
                        if let Some(bytes) = self.core.sync_feed_chunk_bytes(feed, item.hash)? {
                            self.remote.put_chunk_sync(feed, item.hash, &bytes)?;
                            self.core.sync_feed_chunk_uploaded(feed, item.hash)?;
                        }
                    }
                    FeedChunkKind::Copy => {
                        self.remote.copy_chunk_sync(feed, item.hash)?;
                        self.core.sync_feed_chunk_uploaded(feed, item.hash)?;
                    }
                    FeedChunkKind::Download => match self.remote.get_chunk_sync(feed, item.hash)? {
                        Some(bytes) => {
                            self.core.sync_feed_chunk_received(feed, item.hash, bytes)?
                        }
                        None => self.core.sync_feed_chunk_unavailable(feed, item.hash)?,
                    },
                }
            }
            if count < 8 {
                break;
            }
        }
        Ok(())
    }

    // ----- edits ----------------------------------------------------------------------------

    pub(crate) fn write(&mut self, page: PageId, text: &str) {
        self.edits += 1;
        write_text(&mut self.core, page, text, self.edits);
    }

    pub(crate) fn texts(&mut self, page: PageId) -> Vec<String> {
        let mut out = Vec::new();
        collect_text(&page_content(&mut self.core, page), &mut out);
        out
    }

    pub(crate) fn events(&mut self) -> Vec<Event> {
        let mut all = std::mem::take(&mut self.log);
        all.extend(self.core.drain_events());
        all
    }
}

fn collect_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(text)) = map.get("text") {
                out.push(text.clone());
            }
            for child in map.values() {
                collect_text(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_text(child, out);
            }
        }
        _ => {}
    }
}

// ----- scenarios' building blocks ----------------------------------------------------------------

/// Shares a space as `owner` the way `SpaceShare` will start: the feed, its key, the `Create`; the
/// owner's first cycles bring the documents in.
pub(crate) fn share(owner: &mut Dev, space: SpaceId, name: &str, profile: &str) -> FeedId {
    let feed = owner
        .core
        .mint_create(space, name, SpaceSettings::default(), profile)
        .unwrap();
    owner.core.chunks_follow_space_to_feed(space).unwrap();
    // Everything of the space goes into the feed at the owner's next cycles.
    owner.cycle().unwrap();
    owner.cycle().unwrap();
    feed
}

/// What an invitee holds after an `Invite`: the secret's derived key and the bundle's space key.
struct Invited {
    id: kb_types::InviteId,
    signing: kb_crypto::SigningKey,
    epoch: u32,
    key: kb_keys::space_keys::SpaceKey,
}

/// Adds `member` to the space's feed with `role`: the owner writes an `Invite`, the member opens it,
/// holds the space key as a bundle gives it and posts a `Join`; then the member bootstraps.
pub(crate) fn add_member(
    owner: &mut Dev,
    member: &mut Dev,
    feed: FeedId,
    space: SpaceId,
    role: MemberRole,
    profile: &str,
) {
    let invited = invite(owner, member, feed, role);
    join(member, invited, feed, space, profile);
}

/// The owner writes an `Invite` for `member` and the bundle's key is the feed's current one.
fn invite(owner: &mut Dev, member: &Dev, feed: FeedId, role: MemberRole) -> Invited {
    let secret = InviteSecret::generate(&SeededRng::new(7000 + member.edits));
    let id = invite_id(&secret);
    let signing = invite_signing_key(&secret);
    let expires_at = owner.remote.now_ms() + 7 * DAY_MS;
    owner
        .core
        .membership_mint(
            feed,
            EntryBody::Invite {
                invite_id: id,
                invite_pub: signing.verifying_key(),
                role,
                expires_at,
                bound_account: None,
                verified: false,
                enc_bundle: None,
            },
            Private::None,
            None,
        )
        .unwrap();
    owner.cycle().unwrap();
    // The bundle's key and epoch.
    let epoch = owner.core.tentative_state(feed).unwrap().unwrap().epoch;
    let key = owner.core.feed_sk(feed, epoch).unwrap().unwrap();
    Invited {
        id,
        signing,
        epoch,
        key,
    }
}

/// The invitee opens the invite, holds the bundle's key and posts a `Join`, then bootstraps.
fn join(member: &mut Dev, invited: Invited, feed: FeedId, space: SpaceId, profile: &str) {
    join_with(member, invited, feed, Some(space), profile);
}

/// [`join`], the space known (the code flow's bundle names it) or not (the link flow's does not: the
/// first key record of the feed will).
fn join_with(
    member: &mut Dev,
    invited: Invited,
    feed: FeedId,
    space: Option<SpaceId>,
    profile: &str,
) {
    let Invited {
        id,
        signing,
        epoch,
        key,
    } = invited;
    // The member needs an account on the server to open the invite and join.
    member.cycle().unwrap();
    let epoch_of_server = member.remote.server_epoch();
    let opened = member
        .remote
        .invite_open_sync(id, &sign_open(&signing, id, epoch_of_server))
        .unwrap();
    member.core.feed_register_shared(feed, space).unwrap();
    member.core.feed_store_sk(feed, epoch, &key).unwrap();
    let tail = opened.log.last().unwrap();
    let head = MembershipEntry::decode(&tail.bytes).unwrap().head();
    // The fold needs the device logs of everybody the log names: fetch them and fold again, until
    // the whole log is folded.
    for _ in 0..6 {
        member
            .core
            .sync_membership_fetched(
                feed,
                &MembershipPage {
                    entries: opened
                        .log
                        .iter()
                        .map(|item| MembershipItem {
                            cursor: item.cursor,
                            bytes: item.bytes.clone(),
                        })
                        .collect(),
                    head,
                    more: false,
                },
            )
            .unwrap();
        member.core.shared.stalled.clear();
        let folded = member
            .core
            .feed_timeline(feed)
            .unwrap()
            .map_or(0, kb_keys::membership::Timeline::len);
        if folded == opened.log.len() {
            break;
        }
        member.fetch_logs().unwrap();
    }
    let account = member.account();
    let log = member.core.security.log.as_ref().unwrap();
    let last = log.entries().last().unwrap();
    let log_head = ChainHead {
        seq: last.seq,
        hash: last.hash(),
    };
    let proof = sign_join_proof(&signing, feed, id, account, &log_head);
    member
        .core
        .membership_mint(
            feed,
            EntryBody::Join {
                invite_id: id,
                proof,
                account,
                log: log_head,
            },
            Private::Join {
                profile_name: profile.to_owned(),
            },
            Some((epoch, key)),
        )
        .unwrap();
    if let Some(space) = space {
        member.core.set_shared_space_for_test(space, feed);
    }
    member.cycle().unwrap();
    member.cycle().unwrap();
}

impl Core {
    /// Writes `sharedSpaces[space]` the way the join will (the test's other devices find it).
    fn set_shared_space_for_test(&mut self, space: SpaceId, feed: FeedId) {
        self.ensure_account().unwrap();
        self.docs
            .account
            .as_ref()
            .unwrap()
            .model
            .set_shared_space(space, feed)
            .unwrap();
        self.persist_account().unwrap();
    }
}

fn outline_of(dev: &mut Dev, space: SpaceId) -> String {
    outline(&tree(&mut dev.core, space))
}

fn role_of(dev: &mut Dev, space: SpaceId) -> Option<MemberRole> {
    dev.core.role_in_space(space).unwrap()
}

// ----- tests ----------------------------------------------------------------------------------------

#[test]
fn the_personal_feed_converges_two_devices_through_the_v2_host_methods() {
    let server = server();
    let (mut a, kit) = Dev::first("a", 1, &server);
    a.configure();
    let space = crate::test_support::spaces(&mut a.core)[0].id;
    let page = create_page(&mut a.core, space, None, "Notes").id;
    a.write(page, "from device a");
    a.cycle().unwrap();
    let mut b = Dev::from_backup("b", 2, &server, &mut a, &kit);
    b.configure();
    b.cycle().unwrap();
    a.write(page, "second line");
    a.cycle().unwrap();
    b.cycle().unwrap();
    assert_eq!(b.texts(page), a.texts(page));
    assert_eq!(a.texts(page).len(), 2);
    a.cycle().unwrap();
    let info = a.core.sync_info().unwrap();
    assert_eq!(info.feeds.len(), 1, "the personal feed is the only feed");
    assert_eq!(info.pending_entries, 0);
}

#[test]
fn a_member_bootstraps_a_shared_space_and_edits_converge() {
    let server = server();
    let (mut alice, _) = Dev::first("alice", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    alice.configure();
    bob.configure();
    let space = create_space(&mut alice.core, "Project").id;
    let plan = create_page(&mut alice.core, space, None, "Plan").id;
    let notes = create_page(&mut alice.core, space, Some(plan), "Notes").id;
    alice.write(plan, "alice wrote the plan");
    alice.write(notes, "alice wrote notes");
    let feed = share(&mut alice, space, "Project", "Alice");
    add_member(&mut alice, &mut bob, feed, space, MemberRole::Editor, "Bob");
    Dev::settle(&mut [&mut alice, &mut bob], 2);

    assert_eq!(outline_of(&mut bob, space), outline_of(&mut alice, space));
    assert_eq!(bob.texts(plan), alice.texts(plan));
    assert_eq!(bob.texts(notes), vec!["alice wrote notes".to_owned()]);
    assert_eq!(role_of(&mut bob, space), Some(MemberRole::Editor));
    assert_eq!(role_of(&mut alice, space), Some(MemberRole::Owner));

    // Both edit; both see both.
    bob.write(plan, "bob added a line");
    alice.write(notes, "alice again");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    alice.cycle().unwrap();
    assert_eq!(bob.texts(plan), alice.texts(plan));
    assert_eq!(bob.texts(notes), alice.texts(notes));
    assert!(alice.texts(plan).contains(&"bob added a line".to_owned()));
    assert!(bob.texts(notes).contains(&"alice again".to_owned()));

    // The panel's view.
    let members = alice.core.members_info(space).unwrap();
    assert_eq!(members.members.len(), 2);
    assert_eq!(members.epoch, 1);
    let rejected = alice.core.db.rejected_list(feed, 10).unwrap();
    let described: Vec<String> = rejected
        .iter()
        .map(|r| format!("cursor {} reason {:?} doc {}", r.cursor, r.reason, r.doc_id))
        .collect();
    assert_eq!(members.refused_total, 0, "{described:?}");
    let names: Vec<&str> = members.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["Alice", "Bob"]);
    assert!(members.members[1].safety_number.is_some());
    assert_eq!(members.invites.len(), 1);
    let breakdown = alice.pending_breakdown(feed);
    assert_eq!(
        alice.core.sync_info().unwrap().pending_entries,
        0,
        "{breakdown}"
    );
    let _ = (
        ErrorCode::Crypto,
        ArchiveReason::Removed,
        DocId::from_bytes([0; 16]),
        Adversary::AcceptAll,
    );
}

impl Dev {
    /// What `SyncInfo.pending_entries` counts, listed.
    #[allow(dead_code)]
    fn pending_breakdown(&self, feed: FeedId) -> String {
        use kb_storage::{DocQueries, SyncQueries};
        let personal = self.core.personal_feed().unwrap().unwrap();
        let shared = self.core.shared_space_map().unwrap();
        let mut out = String::new();
        for row in self.core.db.docs_all().unwrap() {
            if Core::feed_of_doc(&row, &shared, personal) != Some(feed) {
                continue;
            }
            let pushed = self
                .core
                .db
                .sync_doc_state_get_in(feed, row.doc_id)
                .unwrap()
                .map_or(0, |s| s.pushed_up_to_seq);
            let last = self
                .core
                .db
                .update_stats(row.doc_id)
                .unwrap()
                .last_seq
                .unwrap_or(0);
            if last > pushed {
                out.push_str(&format!(
                    "{:?} {} last={last} pushed={pushed}; ",
                    row.kind, row.doc_id
                ));
            }
        }
        out.push_str(&format!(
            "keyring={} membership={}",
            self.core
                .keyring_unpublished_for(feed, &shared)
                .unwrap()
                .len(),
            self.core.unconfirmed_membership_rows(feed).unwrap().len()
        ));
        out
    }
}

// ----- membership changes the way the request side will mint them ------------------------------------

fn remove_member(owner: &mut Dev, feed: FeedId, target: kb_types::AccountId) {
    let state = owner.core.tentative_state(feed).unwrap().unwrap();
    let remaining: Vec<kb_types::AccountId> = state
        .members
        .iter()
        .map(|m| m.account)
        .filter(|account| *account != target)
        .collect();
    let (fields, key) = owner
        .core
        .build_rekey_fields(feed, &remaining)
        .unwrap()
        .expect("the members' device logs are here");
    let epoch = fields.new_epoch;
    owner
        .core
        .membership_mint(
            feed,
            EntryBody::Remove {
                target,
                rekey: fields,
            },
            Private::None,
            Some((epoch, key)),
        )
        .unwrap();
}

fn set_role(owner: &mut Dev, feed: FeedId, target: kb_types::AccountId, role: MemberRole) {
    owner
        .core
        .membership_mint(
            feed,
            EntryBody::RoleSet { target, role },
            Private::None,
            None,
        )
        .unwrap();
}

fn leave(member: &mut Dev, feed: FeedId) {
    member
        .core
        .membership_mint(feed, EntryBody::Leave, Private::None, None)
        .unwrap();
}

fn server_adversary(dev: &Dev, mode: Option<Adversary>) {
    dev.remote.set_adversary(mode);
}

/// Alice shares "Project" with two pages and Bob joins with `role`.
struct Pair {
    alice: Dev,
    bob: Dev,
    space: SpaceId,
    feed: FeedId,
    plan: PageId,
    notes: PageId,
}

fn pair(role: MemberRole) -> Pair {
    let server = server();
    let (mut alice, _) = Dev::first("alice", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    alice.configure();
    bob.configure();
    let space = create_space(&mut alice.core, "Project").id;
    let plan = create_page(&mut alice.core, space, None, "Plan").id;
    let notes = create_page(&mut alice.core, space, Some(plan), "Notes").id;
    alice.write(plan, "base line");
    alice.write(notes, "alice notes");
    let feed = share(&mut alice, space, "Project", "Alice");
    add_member(&mut alice, &mut bob, feed, space, role, "Bob");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    alice.core.drain_events();
    bob.core.drain_events();
    Pair {
        alice,
        bob,
        space,
        feed,
        plan,
        notes,
    }
}

/// An entry that claims `account`'s `device` and is signed by `signer`.
fn forged_entry(
    feed: FeedId,
    account: kb_types::AccountId,
    device: kb_types::DeviceId,
    doc: DocId,
    signer: &kb_keys::DeviceKeys,
) -> Vec<u8> {
    let header = kb_sync::v2::EntryHeader {
        feed,
        account_id: account,
        doc_id: doc,
        device_id: device,
        device_seq: 1,
        key_epoch: 1,
    };
    let body = kb_sync::EntryBody {
        engine: kb_doc::ENGINE_LORO,
        format_version: kb_doc::FORMAT_VERSION,
        min_reader_version: kb_doc::MIN_READER_VERSION,
        doc_kind: 1,
        updates: vec![vec![1, 2, 3]],
        version_vector: Vec::new(),
    };
    kb_sync::v2::seal_entry(
        &header,
        &body,
        &kb_crypto::Secret32::generate(&SeededRng::new(5)),
        signer.signing(),
        &SeededRng::new(6),
    )
}

#[test]
fn a_viewers_forced_entry_is_quarantined_and_counted_by_the_receivers() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Viewer);
    assert_eq!(role_of(&mut bob, space), Some(MemberRole::Viewer));
    // A Viewer's own outbox is empty, whatever its pages hold (the request refuses the write, so a
    // hostile client writes past it).
    assert!(bob.core.page_doc_import(plan, &[1]).is_err());
    bob.core.shared.force_push = true;
    bob.write(plan, "a viewer should not type this");
    bob.core.shared.force_push = false;
    assert!(bob.core.sync_feed_outbox(feed).unwrap().is_empty());
    // Forced past that, the server still refuses the append of a Viewer.
    bob.core.shared.force_push = true;
    let outbox = bob.core.sync_feed_outbox(feed).unwrap();
    assert!(!outbox.is_empty());
    assert!(matches!(
        bob.remote.append_sync(feed, &outbox[0].bytes),
        Err(kb_sync::SyncError::Forbidden)
    ));
    // A server that accepts anything lets it in; the receivers refuse it.
    server_adversary(&bob, Some(Adversary::AcceptAll));
    bob.cycle().unwrap();
    server_adversary(&bob, None);
    bob.core.shared.force_push = false;
    alice.events();
    alice.cycle().unwrap();
    assert!(
        !alice
            .texts(plan)
            .contains(&"a viewer should not type this".to_owned()),
        "the quarantined entry is not applied"
    );
    let events = alice.events();
    assert!(events.contains(&Event::SpaceMembersChanged { space_id: space }));
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.refused_total, 1);
    let bob_row = info.members.iter().find(|m| m.name == "Bob").unwrap();
    assert_eq!(bob_row.refused_changes, 1);
    assert!(
        !info.server_flagged,
        "a member's item is not the server's fault"
    );
    // The chain advanced past it: Alice goes on.
    alice.write(plan, "alice carries on");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(bob.texts(plan).contains(&"alice carries on".to_owned()));
}

#[test]
fn an_item_of_a_non_member_is_quarantined_and_flags_the_server() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let server = alice.remote.clone();
    // Carol has an account on the server and no part in the space.
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    carol.cycle().unwrap();
    let (account, device) = carol.core.sync_identity().unwrap().unwrap();
    let bytes = forged_entry(
        feed,
        account,
        device,
        DocId::for_page(plan),
        &carol.core.security.device_keys,
    );
    server.set_adversary(Some(Adversary::AcceptAll));
    carol.remote.append_sync(feed, &bytes).unwrap();
    server.set_adversary(None);
    alice.cycle().unwrap();
    bob.cycle().unwrap();
    for dev in [&mut alice, &mut bob] {
        let info = dev.core.members_info(space).unwrap();
        assert_eq!(info.refused_total, 1, "{}", dev.name);
        assert!(
            info.server_flagged,
            "{}: the server let a non-member in",
            dev.name
        );
    }
    // The feed is not stuck.
    alice.write(plan, "still working");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(bob.texts(plan).contains(&"still working".to_owned()));
}

#[test]
fn a_forged_signature_stops_the_feed_like_a_forged_entry_of_v1() {
    let Pair {
        mut alice,
        bob,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let server = alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    carol.cycle().unwrap();
    // An entry that says it is Bob's and is signed by Carol's device.
    let (bob_account, bob_device) = bob.core.sync_identity().unwrap().unwrap();
    let mut bytes = forged_entry(
        feed,
        bob_account,
        bob_device,
        DocId::for_page(plan),
        &carol.core.security.device_keys,
    );
    server.set_adversary(Some(Adversary::AcceptAll));
    carol.remote.append_sync(feed, &bytes).unwrap();
    server.set_adversary(None);
    let error = alice.cycle().unwrap_err();
    match error {
        SimError::Api(api) => assert_eq!(api.code, ErrorCode::Crypto),
        other => panic!("{other:?}"),
    }
    // The same stop on every later cycle.
    assert!(alice.cycle().is_err());
    bytes.clear();
}

#[test]
fn a_removed_members_later_entry_is_refused_and_the_earlier_ones_stay() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    // Bob writes while he is a member and everybody has it.
    bob.write(plan, "bob before the removal");
    Dev::settle(&mut [&mut bob, &mut alice], 2);
    assert!(
        alice
            .texts(plan)
            .contains(&"bob before the removal".to_owned())
    );
    // Bob goes offline and writes again; Alice removes him.
    bob.write(plan, "bob after the removal");
    let bob_account = bob.account();
    remove_member(&mut alice, feed, bob_account);
    alice.cycle().unwrap();
    // The server refuses the removed account; a server that accepts anything lets the entry in.
    server_adversary(&bob, Some(Adversary::AcceptAll));
    bob.core.shared.force_push = true;
    let outbox = bob.core.sync_feed_outbox(feed).unwrap();
    assert!(!outbox.is_empty());
    for entry in &outbox {
        bob.remote.append_sync(feed, &entry.bytes).unwrap();
    }
    server_adversary(&bob, None);
    bob.core.shared.force_push = false;

    alice.events();
    alice.cycle().unwrap();
    let texts = alice.texts(plan);
    assert!(texts.contains(&"bob before the removal".to_owned()));
    assert!(
        !texts.contains(&"bob after the removal".to_owned()),
        "an entry after the Remove is refused"
    );
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.members.len(), 1);
    assert_eq!(info.epoch, 2, "the removal carries its Rekey");
    assert!(
        info.server_flagged,
        "a removed account's item is not a member's"
    );
    assert_eq!(info.refused_total, 1);

    // Bob learns it from the feed: his copy becomes an archive.
    bob.events();
    bob.cycle().unwrap();
    let events = bob.events();
    assert!(events.contains(&Event::SpaceRemoved {
        space_id: space,
        reason: ArchiveReason::Removed
    }));
    assert_eq!(
        bob.core.feed_archived(feed).unwrap(),
        Some(ArchiveReason::Removed)
    );
    assert!(
        bob.core
            .sync_feeds()
            .unwrap()
            .iter()
            .all(|w| w.feed != feed)
    );
    // He keeps what he had, and nothing newer: no key of the new epoch.
    assert!(
        bob.texts(plan)
            .contains(&"bob before the removal".to_owned())
    );
    assert!(bob.core.feed_sk(feed, 1).unwrap().is_some());
    assert!(bob.core.feed_sk(feed, 2).unwrap().is_none());
    alice.write(plan, "written after the removal");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(
        !bob.texts(plan)
            .contains(&"written after the removal".to_owned())
    );
    assert!(
        alice
            .texts(plan)
            .contains(&"written after the removal".to_owned())
    );
    // The server would refuse him anyway, past his Remove.
    let at = bob.core.feed_row(feed).unwrap().cursor;
    assert!(matches!(
        bob.remote.changes_sync(feed, at, 10),
        Err(kb_sync::SyncError::NotFound | kb_sync::SyncError::Forbidden)
    ));
}

#[test]
fn a_leave_asks_for_a_rekey_and_the_next_writer_mints_it() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    leave(&mut bob, feed);
    bob.cycle().unwrap();
    assert!(
        alice.remote.pending_rekey(feed),
        "the server waits for a Rekey after a Leave"
    );
    // Alice writes: the append is refused until the Rekey, which her device mints.
    alice.write(plan, "alice after bob left");
    alice.cycle().unwrap();
    alice.cycle().unwrap();
    assert!(
        !alice.remote.pending_rekey(feed),
        "the Rekey cleared the flag"
    );
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.epoch, 2);
    assert_eq!(info.members.len(), 1);
    assert!(!info.pending_rekey);
    // Her write went through under the new epoch.
    alice.cycle().unwrap();
    assert_eq!(alice.core.sync_info().unwrap().pending_entries, 0);
    let items = alice.remote.inspect_feed(feed);
    assert!(items.len() > 4);
    // The leaver's copy is an archive that cannot read the new epoch.
    assert_eq!(
        bob.core.feed_archived(feed).unwrap(),
        Some(ArchiveReason::Removed)
    );
    assert!(bob.core.feed_sk(feed, 2).unwrap().is_none());
}

#[test]
fn a_stale_epoch_is_resealed_under_the_one_the_pull_taught() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    bob.write(plan, "bob wrote under epoch one");
    let stale = bob.core.sync_feed_outbox(feed).unwrap();
    assert!(!stale.is_empty());
    // Alice rekeys the space (a plain Rekey of all members).
    alice.core.mint_rekey(feed).unwrap().expect("minted");
    alice.cycle().unwrap();
    alice.cycle().unwrap();
    assert_eq!(alice.core.members_info(space).unwrap().epoch, 2);
    // Bob's entries were sealed under epoch one: the server refuses them.
    assert!(stale.iter().any(|entry| matches!(
        bob.remote.append_sync(feed, &entry.bytes),
        Err(kb_sync::SyncError::StaleEpoch)
    )));
    // His next cycle learns the Rekey, opens his box, reseals; Alice gets the text.
    bob.cycle().unwrap();
    bob.cycle().unwrap();
    assert_eq!(bob.core.members_info(space).unwrap().epoch, 2);
    assert!(bob.core.feed_sk(feed, 2).unwrap().is_some());
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(
        alice
            .texts(plan)
            .contains(&"bob wrote under epoch one".to_owned())
    );
    assert_eq!(bob.core.sync_info().unwrap().pending_entries, 0);
}

#[test]
fn a_truncating_snapshot_is_refused_disputed_and_republished() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    // The state of the page before Bob's line, as a hostile snapshot will carry it.
    let before = alice.core.page_doc_open(plan).unwrap();
    let truncated = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &before);
    bob.write(plan, "bob's line that the snapshot omits");
    Dev::settle(&mut [&mut bob, &mut alice], 2);
    assert!(
        alice
            .texts(plan)
            .contains(&"bob's line that the snapshot omits".to_owned())
    );
    // Bob's device publishes a snapshot that covers everything and holds less.
    let covers = bob.core.feed_row(feed).unwrap().cursor;
    let doc = DocId::for_page(plan);
    let snapshot = bob
        .core
        .test_seal_snapshot(feed, doc, truncated, covers)
        .unwrap();
    bob.remote.put_snapshot_sync(feed, &snapshot).unwrap();
    alice.events();
    alice.cycle().unwrap();
    // Alice checked it against what she applied: refused, disputed, the Owners told.
    let events = alice.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::SnapshotDisputed { space_id, .. } if *space_id == space)),
        "{events:?}"
    );
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.disputes.len(), 1);
    assert_eq!(info.disputes[0].page_id, Some(plan));
    assert_eq!(info.refused_total, 1);
    assert!(
        alice
            .texts(plan)
            .contains(&"bob's line that the snapshot omits".to_owned())
    );
    // Her next cycle republishes a full snapshot, which replaces the disputed one.
    alice.cycle().unwrap();
    let latest = alice
        .remote
        .get_snapshot_sync(feed, doc)
        .unwrap()
        .expect("a snapshot");
    let header = kb_sync::v2::peek_snapshot(&latest).unwrap();
    assert_eq!(header.account_id, alice.account());
    // A member who joins later reads Bob's line.
    let server = alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    add_member(
        &mut alice,
        &mut carol,
        feed,
        space,
        MemberRole::Viewer,
        "Carol",
    );
    Dev::settle(&mut [&mut alice, &mut carol], 2);
    assert!(
        carol
            .texts(plan)
            .contains(&"bob's line that the snapshot omits".to_owned())
    );
    assert_eq!(carol.texts(plan), alice.texts(plan));
}

#[test]
fn a_demoted_device_forks_out_its_unsent_changes_and_rebuilds_from_the_feed() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        notes,
    } = pair(MemberRole::Editor);
    let bob_account = bob.account();
    // Alice demotes Bob while he is away; Bob types on.
    set_role(&mut alice, feed, bob_account, MemberRole::Commenter);
    alice.cycle().unwrap();
    bob.write(plan, "bob types before he knows");
    bob.write(notes, "and here too");
    bob.events();
    // His push is accepted by the server (a Commenter may append) and refused by the members.
    bob.cycle().unwrap();
    let events = bob.events();
    assert!(events.contains(&Event::SpaceRoleChanged {
        space_id: space,
        role: MemberRole::Commenter,
        previous: MemberRole::Editor
    }));
    let copied = events
        .iter()
        .find_map(|e| match e {
            Event::UnsentChangesCopied {
                space_id,
                target_space_id,
                page_ids,
            } if *space_id == space => Some((*target_space_id, page_ids.clone())),
            _ => None,
        })
        .expect("fork-out ran");
    assert_eq!(copied.1.len(), 2);
    // The copies are pages of a personal space with the text Bob typed.
    let target = copied.0;
    assert_ne!(target, space);
    let titles = outline(&tree(&mut bob.core, target));
    assert!(titles.contains("Unsent changes \u{2013} Plan"), "{titles}");
    assert!(titles.contains("Unsent changes \u{2013} Notes"), "{titles}");
    let copy_texts: Vec<Vec<String>> = copied.1.iter().map(|page| bob.texts(*page)).collect();
    assert!(
        copy_texts
            .iter()
            .any(|t| t.contains(&"bob types before he knows".to_owned()))
    );
    assert!(
        copy_texts
            .iter()
            .any(|t| t.contains(&"and here too".to_owned()))
    );
    // The shared pages are rebuilt from the feed: his typing is not in them any more, anywhere.
    Dev::settle(&mut [&mut bob, &mut alice], 2);
    assert_eq!(bob.texts(plan), alice.texts(plan));
    assert_eq!(bob.texts(notes), alice.texts(notes));
    assert!(
        !alice
            .texts(plan)
            .contains(&"bob types before he knows".to_owned())
    );
    assert!(
        !bob.texts(plan)
            .contains(&"bob types before he knows".to_owned())
    );
    let info = alice.core.members_info(space).unwrap();
    assert!(info.refused_total >= 2);
    assert_eq!(
        info.members
            .iter()
            .find(|m| m.name == "Bob")
            .map(|m| m.role),
        Some(MemberRole::Commenter)
    );
    // Nothing is left to send and a further cycle changes nothing.
    Dev::settle(&mut [&mut bob, &mut alice], 2);
    assert_eq!(bob.texts(plan), alice.texts(plan));
    let titles_after = outline(&tree(&mut bob.core, target));
    assert_eq!(
        titles_after.matches("Unsent changes").count(),
        2,
        "fork-out ran once"
    );
}

#[test]
fn revoking_a_device_rekeys_every_shared_feed_once() {
    let server = server();
    let (mut a1, kit) = Dev::first("a1", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    a1.configure();
    bob.configure();
    let space = create_space(&mut a1.core, "Project").id;
    let plan = create_page(&mut a1.core, space, None, "Plan").id;
    a1.write(plan, "first");
    let feed = share(&mut a1, space, "Project", "Alice");
    add_member(&mut a1, &mut bob, feed, space, MemberRole::Editor, "Bob");
    // A second device of Alice's account opens the space through the personal keyring and the
    // account document.
    a1.cycle().unwrap();
    let mut a2 = Dev::from_backup("a2", 4, &server, &mut a1, &kit);
    a2.configure();
    Dev::settle(&mut [&mut a2, &mut a1, &mut bob], 3);
    assert_eq!(a2.texts(plan), a1.texts(plan));
    assert_eq!(a2.core.members_info(space).unwrap().epoch, 1);
    // Bob was told once that Alice's account added a device with its recovery kit.
    let notices: Vec<Event> = bob
        .events()
        .into_iter()
        .filter(|event| matches!(event, Event::MemberDeviceViaKit { .. }))
        .collect();
    assert_eq!(
        notices,
        vec![Event::MemberDeviceViaKit {
            space_id: space,
            account_id: a1.account(),
            name: "Alice".to_owned()
        }]
    );
    let device_count = a1
        .core
        .members_info(space)
        .unwrap()
        .members
        .iter()
        .find(|m| m.is_me)
        .map(|m| m.device_count);
    assert_eq!(device_count, Some(2));

    // a1 revokes a2: the root key rotates and the feed gets one Rekey that leaves a2 out.
    let a2_device = a2.core.device.device_id;
    a1.core
        .dispatch(kb_types::Request::DeviceRevoke {
            device_id: a2_device,
            passphrase: None,
        })
        .unwrap();
    Dev::settle(&mut [&mut a1, &mut bob], 3);
    let info = a1.core.members_info(space).unwrap();
    assert_eq!(info.epoch, 2, "one Rekey");
    assert!(!a1.remote.pending_rekey(feed));
    assert_eq!(
        info.members
            .iter()
            .find(|m| m.is_me)
            .map(|m| m.device_count),
        Some(1)
    );
    // Bob follows and reads what a1 writes under the new epoch.
    a1.write(plan, "after the revocation");
    Dev::settle(&mut [&mut a1, &mut bob], 3);
    assert_eq!(bob.core.members_info(space).unwrap().epoch, 2);
    assert!(bob.texts(plan).contains(&"after the revocation".to_owned()));
    // The revoked device is out: the server refuses it and no box of the new epoch is for it.
    assert!(a2.cycle().is_err() || a2.core.feed_sk(feed, 2).unwrap().is_none());
}

#[test]
fn a_members_new_device_opens_the_space_through_the_personal_keyring() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        notes,
        ..
    } = pair(MemberRole::Editor);
    let server = alice.remote.clone();
    let mut bob2 = Dev::fresh("bob2", 5, &server, &mut bob);
    bob2.configure();
    Dev::settle(&mut [&mut bob2, &mut bob, &mut alice], 3);
    // The new device found `sharedSpaces` in the account document and the space keys in the
    // personal keyring, then bootstrapped the feed.
    assert_eq!(role_of(&mut bob2, space), Some(MemberRole::Editor));
    assert_eq!(outline_of(&mut bob2, space), outline_of(&mut alice, space));
    assert_eq!(bob2.texts(plan), alice.texts(plan));
    assert_eq!(bob2.texts(notes), alice.texts(notes));
    // It writes and the others read.
    bob2.write(notes, "from the new device");
    Dev::settle(&mut [&mut bob2, &mut alice, &mut bob], 2);
    assert!(
        alice
            .texts(notes)
            .contains(&"from the new device".to_owned())
    );
    assert_eq!(bob.texts(notes), alice.texts(notes));
    // The member panel counts the devices.
    let info = alice.core.members_info(space).unwrap();
    let bob_row = info.members.iter().find(|m| m.name == "Bob").unwrap();
    assert_eq!(bob_row.device_count, 2);
    assert_eq!(info.refused_total, 0);
}

#[test]
fn a_member_who_joined_with_an_older_bundle_gets_a_grant() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let server = alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    // The invite is written (and its bundle made) at epoch one; the space is re-keyed; then Carol
    // joins with the old bundle.
    let invited = invite(&mut alice, &carol, feed, MemberRole::Editor);
    assert_eq!(invited.epoch, 1);
    alice.core.mint_rekey(feed).unwrap().expect("minted");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(alice.core.members_info(space).unwrap().epoch, 2);
    alice.write(plan, "written at epoch two");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    join(&mut carol, invited, feed, space, "Carol");
    // She holds the key of epoch one only until a member boxes the current one for her.
    assert!(carol.core.feed_sk(feed, 2).unwrap().is_none());
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 4);
    assert!(
        carol.core.feed_sk(feed, 2).unwrap().is_some(),
        "a Grant arrived"
    );
    assert_eq!(carol.core.members_info(space).unwrap().epoch, 2);
    assert!(
        carol
            .texts(plan)
            .contains(&"written at epoch two".to_owned())
    );
    assert_eq!(carol.texts(plan), alice.texts(plan));
    // Exactly one Grant for her, whoever minted it.
    let grants = alice
        .remote
        .inspect_feed(feed)
        .into_iter()
        .filter(|(_, bytes)| {
            kb_keys::membership::MembershipEntry::peek(bytes)
                .is_ok_and(|h| h.kind() == kb_keys::membership::EntryKind::Grant)
        })
        .count();
    assert_eq!(grants, 1);
}

#[test]
fn a_rekey_that_boxes_a_revoked_device_is_repaired_by_its_accounts_own_device() {
    let server = server();
    let (mut a1, kit) = Dev::first("a1", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    a1.configure();
    bob.configure();
    let space = create_space(&mut a1.core, "Project").id;
    let plan = create_page(&mut a1.core, space, None, "Plan").id;
    a1.write(plan, "first");
    let feed = share(&mut a1, space, "Project", "Alice");
    add_member(&mut a1, &mut bob, feed, space, MemberRole::Editor, "Bob");
    a1.cycle().unwrap();
    let mut a2 = Dev::from_backup("a2", 4, &server, &mut a1, &kit);
    a2.configure();
    Dev::settle(&mut [&mut a2, &mut a1, &mut bob], 3);
    let epoch_before = a1.core.members_info(space).unwrap().epoch;

    // a1 revokes a2 but has not told the server yet; meanwhile Bob re-keys with the log as the
    // server has it, which still lists a2.
    let a2_device = a2.core.device.device_id;
    a1.core
        .dispatch(kb_types::Request::DeviceRevoke {
            device_id: a2_device,
            passphrase: None,
        })
        .unwrap();
    bob.core.mint_rekey(feed).unwrap().expect("minted");
    bob.cycle().unwrap();
    assert_eq!(
        bob.remote
            .inspect_feed(feed)
            .iter()
            .filter(|(_, b)| kb_keys::membership::MembershipEntry::peek(b)
                .is_ok_and(|h| h.kind() == kb_keys::membership::EntryKind::Rekey))
            .count(),
        1,
        "Bob's Rekey is in the feed"
    );
    // a1 now publishes its log; its own Rekey loses the race, it sees that Bob's Rekey boxes a2
    // (its head of this account is older than the revocation) and mints a fresh one.
    Dev::settle(&mut [&mut a1, &mut bob], 3);
    let info = a1.core.members_info(space).unwrap();
    assert_eq!(
        info.epoch,
        epoch_before + 2,
        "Bob's Rekey, then a1's repair"
    );
    assert!(!a1.remote.pending_rekey(feed));
    assert_eq!(
        bob.core.members_info(space).unwrap().epoch,
        epoch_before + 2
    );
    a1.write(plan, "after the repair");
    Dev::settle(&mut [&mut a1, &mut bob], 3);
    assert!(bob.texts(plan).contains(&"after the repair".to_owned()));
    // The revoked device holds the key of the epoch Bob's Rekey made and not the repaired one.
    assert!(a2.core.feed_sk(feed, epoch_before + 2).unwrap().is_none());
}

#[test]
fn keyring_tombstones_count_only_from_an_owner() {
    let Pair {
        mut alice,
        mut bob,
        space,
        notes,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let notes_doc = DocId::for_page(notes);
    let plan_doc = DocId::for_page(plan);
    // An Editor's tombstone is refused by the others and changes nothing there.
    bob.core
        .keyring_tombstone_docs(space, &[notes_doc])
        .unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 2);
    let subject = kb_keys::KeySubject::Doc {
        doc_id: notes_doc,
        epoch: 1,
    };
    assert!(!alice.core.security.keyring.is_tombstoned(space, &subject));
    assert!(
        alice
            .core
            .security
            .keyring
            .record(space, &subject)
            .is_some()
    );
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.refused_total, 1);
    // An Owner's tombstone is taken by every member.
    alice
        .core
        .keyring_tombstone_docs(space, &[plan_doc])
        .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    let plan_subject = kb_keys::KeySubject::Doc {
        doc_id: plan_doc,
        epoch: 1,
    };
    assert!(
        bob.core
            .security
            .keyring
            .is_tombstoned(space, &plan_subject)
    );
}

#[test]
fn a_server_reset_reposts_the_membership_and_everybody_catches_up() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        notes,
    } = pair(MemberRole::Editor);
    bob.write(notes, "bob before the reset");
    Dev::settle(&mut [&mut bob, &mut alice], 2);
    let members_before = alice.core.members_info(space).unwrap();
    alice.remote.reset_data();
    // Everybody meets a new epoch: the accounts, the membership log and the documents go up again.
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.members.len(), members_before.members.len());
    assert_eq!(info.epoch, members_before.epoch);
    assert_eq!(bob.core.members_info(space).unwrap().members.len(), 2);
    assert_eq!(alice.texts(plan), bob.texts(plan));
    assert!(
        alice
            .texts(notes)
            .contains(&"bob before the reset".to_owned())
    );
    assert_eq!(
        alice
            .remote
            .feed_members(feed)
            .iter()
            .map(|(_, role)| *role)
            .collect::<Vec<_>>(),
        vec![MemberRole::Owner, MemberRole::Editor]
    );
    // And the space works afterwards.
    bob.write(plan, "after the reset");
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert!(alice.texts(plan).contains(&"after the reset".to_owned()));
}

impl Dev {
    /// Uploads `bytes` as an attachment of the space, the way a shell streams a file.
    fn upload_attachment(
        &mut self,
        space_id: SpaceId,
        name: &str,
        bytes: &[u8],
    ) -> kb_types::AttachmentInfo {
        use kb_types::{Request, Response};
        let id = match self
            .core
            .dispatch(Request::AttachmentBegin {
                space_id,
                name: name.into(),
                mime: "application/octet-stream".into(),
            })
            .unwrap()
        {
            Response::AttachmentStarted(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        for piece in bytes.chunks(20_000) {
            self.core.attachment_write(id, piece).unwrap();
        }
        let info = match self
            .core
            .dispatch(Request::AttachmentFinish { attachment_id: id })
            .unwrap()
        {
            Response::Attachment(info) => info,
            other => panic!("unexpected {other:?}"),
        };
        self.core.drain_events();
        info
    }

    fn read_attachment(&mut self, id: kb_types::AttachmentId) -> Result<Vec<u8>, ApiError> {
        let mut out = Vec::new();
        self.core
            .attachment_read(id, None, &mut |piece| {
                out.extend_from_slice(piece);
                Ok(())
            })
            .map(|_| out)
    }
}

#[test]
fn an_attachment_of_a_shared_space_reaches_the_members_through_the_feed() {
    let server = server();
    let (mut alice, _) = Dev::first("alice", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    alice.configure();
    bob.configure();
    let space = create_space(&mut alice.core, "Project").id;
    create_page(&mut alice.core, space, None, "Plan");
    let bytes: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    let before = alice.upload_attachment(space, "before.bin", &bytes);
    // The chunks go to the personal namespace first.
    alice.cycle().unwrap();
    let chunk = alice
        .core
        .db
        .chunk_sync_by_attachment(before.attachment_id)
        .unwrap()[0]
        .clone();
    assert_eq!(chunk.state, crate::transfers::SYNCED);
    // Sharing re-wraps the key under the feed's epoch and copies the chunks server-side.
    let feed = alice
        .core
        .mint_create(space, "Project", SpaceSettings::default(), "Alice")
        .unwrap();
    alice.core.share_rewrap_attachment_keys(space).unwrap();
    alice.core.chunks_follow_space_to_feed(space).unwrap();
    assert_eq!(
        alice
            .core
            .db
            .chunk_sync_get(chunk.hash)
            .unwrap()
            .unwrap()
            .state,
        crate::transfers::COPY
    );
    alice.cycle().unwrap();
    alice.cycle().unwrap();
    assert!(
        alice
            .remote
            .get_chunk_sync(feed, chunk.hash)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        alice
            .core
            .db
            .chunk_sync_get(chunk.hash)
            .unwrap()
            .unwrap()
            .state,
        crate::transfers::SYNCED
    );
    add_member(&mut alice, &mut bob, feed, space, MemberRole::Editor, "Bob");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(bob.read_attachment(before.attachment_id).unwrap(), bytes);

    // A new attachment of a member is wrapped under the feed's epoch and reaches the others.
    let more: Vec<u8> = (0..30_000u32).map(|i| (i % 13) as u8).collect();
    let after = bob.upload_attachment(space, "after.bin", &more);
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(alice.read_attachment(after.attachment_id).unwrap(), more);
}

#[test]
fn closing_a_space_turns_every_copy_into_an_archive() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    alice
        .core
        .membership_mint(feed, EntryBody::Close, Private::None, None)
        .unwrap();
    alice.events();
    bob.events();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    for dev in [&mut alice, &mut bob] {
        let events = dev.events();
        assert!(
            events.contains(&Event::SpaceRemoved {
                space_id: space,
                reason: ArchiveReason::Closed
            }),
            "{}: {events:?}",
            dev.name
        );
        assert_eq!(
            dev.core.feed_archived(feed).unwrap(),
            Some(ArchiveReason::Closed),
            "{}",
            dev.name
        );
        assert!(
            dev.core
                .sync_feeds()
                .unwrap()
                .iter()
                .all(|w| w.feed != feed)
        );
    }
    // What they had stays readable, and the server takes nothing more.
    assert!(!alice.texts(plan).is_empty());
    assert!(alice.remote.append_sync(feed, &[0u8; 8]).is_err());
}

#[test]
fn two_writers_of_one_page_at_a_new_epoch_converge_under_the_first_key_in_feed_order() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    alice.core.mint_rekey(feed).unwrap().expect("minted");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(bob.core.members_info(space).unwrap().epoch, 2);
    // Both write the page before either has seen the other's key record of the new epoch.
    alice.write(plan, "alice at epoch two");
    bob.write(plan, "bob at epoch two");
    alice.cycle().unwrap();
    bob.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let subject = kb_keys::KeySubject::Doc {
        doc_id: DocId::for_page(plan),
        epoch: 2,
    };
    let key_a = alice.core.security.keyring.record(space, &subject).cloned();
    let key_b = bob.core.security.keyring.record(space, &subject).cloned();
    assert!(key_a.is_some());
    assert_eq!(key_a, key_b, "one record, the first in feed order");
    let texts_a = alice.texts(plan);
    assert!(texts_a.contains(&"alice at epoch two".to_owned()));
    assert!(texts_a.contains(&"bob at epoch two".to_owned()));
    assert_eq!(texts_a, bob.texts(plan));
    assert_eq!(alice.core.sync_info().unwrap().parked_items, 0);
    assert_eq!(bob.core.sync_info().unwrap().parked_items, 0);
}

#[test]
fn a_feed_joined_without_its_space_binds_from_the_first_key_record() {
    let Pair {
        mut alice,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let server = alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    // The link flow's bundle holds the key and no space id: the key waits in the stash.
    let invited = invite(&mut alice, &carol, feed, MemberRole::Viewer);
    join_with(&mut carol, invited, feed, None, "Carol");
    Dev::settle(&mut [&mut alice, &mut carol], 3);
    assert_eq!(
        carol.core.feed_row(feed).unwrap().space_id,
        Some(space),
        "the first key record named the space"
    );
    assert_eq!(role_of(&mut carol, space), Some(MemberRole::Viewer));
    assert_eq!(carol.texts(plan), alice.texts(plan));
    assert!(carol.core.feed_sk(feed, 1).unwrap().is_some());
    // A space the device already has is never taken over by a feed that names it.
    let (mut dave, _) = Dev::first("dave", 4, &server);
    let own_space = create_space(&mut dave.core, "Mine").id;
    dave.configure();
    dave.core.feed_register_shared(feed, None).unwrap();
    assert!(!dave.core.feed_bind_space(feed, own_space).unwrap());
    assert_eq!(dave.core.feed_row(feed).unwrap().space_id, None);
}

#[test]
fn a_member_who_joins_after_the_feed_was_pruned_bootstraps_from_the_snapshots() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        notes,
    } = pair(MemberRole::Editor);
    bob.write(notes, "bob before the snapshots settle");
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    // More entries after the snapshots, then everybody acknowledges them.
    alice.write(plan, "alice after the snapshot");
    bob.write(notes, "bob after the snapshot");
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    // Fresh snapshots of both pages (they have covered the start of the feed until now), which
    // everybody acknowledges.
    for doc in [DocId::for_page(plan), DocId::for_page(notes)] {
        alice.core.feed_mark_for_snapshot(feed, doc).unwrap();
    }
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    // A week later the server prunes what the snapshots cover.
    let server = alice.remote.clone();
    server.set_now_ms(SERVER_NOW + 8 * DAY_MS);
    let pruned = server.prune(SERVER_NOW + 8 * DAY_MS);
    assert!(pruned > 0, "the snapshots let the server prune entries");
    // Carol joins afterwards: the membership and the keys are still all there, the content comes
    // from the snapshots.
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    add_member(
        &mut alice,
        &mut carol,
        feed,
        space,
        MemberRole::Commenter,
        "Carol",
    );
    Dev::settle(&mut [&mut carol, &mut alice, &mut bob], 4);
    assert_eq!(carol.texts(plan), alice.texts(plan));
    assert_eq!(carol.texts(notes), alice.texts(notes));
    assert!(
        carol
            .texts(notes)
            .contains(&"bob after the snapshot".to_owned())
    );
    assert_eq!(outline_of(&mut carol, space), outline_of(&mut alice, space));
    assert_eq!(carol.core.sync_info().unwrap().parked_items, 0);
}

#[test]
fn a_head_does_not_acknowledge_a_snapshot_before_it_is_verified() {
    let Pair {
        mut alice,
        mut bob,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    bob.write(plan, "text after the first snapshots");
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    // Bob publishes a snapshot that covers everything he has applied.
    let doc = DocId::for_page(plan);
    let (bytes, covers) = bob.core.sync_feed_make_snapshot(feed, doc).unwrap();
    bob.remote.put_snapshot_sync(feed, &bytes).unwrap();
    // Alice is told of it and has not looked at it yet: her head stays below what it covers.
    let after = alice
        .core
        .sync_snapshot_marks_wanted(feed)
        .unwrap()
        .unwrap();
    let marks = alice.remote.snapshot_marks_sync(feed, after).unwrap();
    let docs = alice
        .core
        .sync_snapshot_marks_received(feed, &marks.marks)
        .unwrap();
    assert_eq!(docs, vec![doc]);
    let held = kb_sync::v2::peek_head(&alice.core.sync_feed_head(feed).unwrap()).unwrap();
    assert!(held.cursor < covers, "{} < {covers}", held.cursor);
    // Verified (here: honestly), it acknowledges everything again.
    alice.core.sync_snapshot_verified(feed, doc, true).unwrap();
    let released = kb_sync::v2::peek_head(&alice.core.sync_feed_head(feed).unwrap()).unwrap();
    assert_eq!(released.cursor, alice.core.feed_row(feed).unwrap().cursor);
}
