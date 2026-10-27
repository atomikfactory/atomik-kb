//! The loop of the driver: what wakes it (`crate::driver::Plan`: `SyncRequested` debounced by the host's
//! hint, `SyncChanged`, the realtime socket's frames (`socket.rs`), the pull timer, the backoff), a
//! cycle's account-level steps (the server and its epoch, the device log, authentication, the recovery
//! blob, the co-members' logs) and the walk over the feeds (`feed.rs` has the steps of one feed). The
//! cycle and its error handling are described in the [module documentation](super).
//!
//! Every core call runs on tokio's blocking pool (it takes the shell's single-writer lock). Nothing is
//! logged but states, counts and cursors: never a token, a nonce, a signature, a body, an id longer than
//! 8 hex characters or an error message from the server.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use kb_sync::SyncError;
use kb_types::sync::{FeedKind, PROTOCOL_VERSION};
use kb_types::{ApiError, ErrorCode, Event, FeedId, SyncState};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::socket::{Socket, next_event};
use super::{SyncHost, SyncRemote};
use crate::NetError;
use crate::driver::{
    Backoff, DriverHandle, IDLE_DEVICE, INVITE_TOKEN_REFUSED, Kind, NO_KIT, Outcome, Plan,
    QUOTA_FULL, RemoteFactory, Step, sleep_until,
};
use crate::http::AuthSigner;
use crate::realtime::RealtimeHandle;

/// Whom the session belongs to: the account and this device.
pub(super) type Who = (kb_types::AccountId, kb_types::DeviceId);

/// What the steps of a cycle share.
#[derive(Clone, Copy)]
pub(super) struct Cx {
    /// The account and this device.
    pub(super) who: Who,
    /// The server's storage epoch (the pages of snapshots the driver builds name it).
    pub(super) epoch: [u8; 16],
    /// Entries and snapshots are uploaded this cycle (a full cycle, or something is pending).
    pub(super) push: bool,
}

/// What a cycle noted about its feeds, for the state it ends in.
#[derive(Debug, Default)]
pub(super) struct Summary {
    /// Some feed's chunk transfers used the whole budget: more are likely waiting.
    pub(super) budget_used: bool,
    /// The server refused an upload for its storage quota.
    pub(super) quota_full: bool,
    /// A membership entry was posted: a feed may be new to the server, so the realtime subscription is
    /// sent again.
    pub(super) membership_posted: bool,
    /// What stopped a feed's step (or a co-member's log), in the order it happened.
    pub(super) problems: Vec<String>,
}

/// The remote for the configured server, and what the driver knows about it.
pub(super) struct Link<R> {
    url: String,
    pub(super) remote: Arc<R>,
    /// The realtime socket (`socket.rs`), once the remote opened one.
    pub(super) realtime: Option<Socket>,
    /// The pause before the next socket after one ended for good (the reference remote's does not
    /// reconnect); reset by a connection.
    pub(super) socket_backoff: Backoff,
    /// Who the session belongs to and the server's storage epoch, as the last cycle found them: what a
    /// pull of one feed outside a cycle needs.
    pub(super) session: Option<(Who, [u8; 16])>,
    /// Feeds whose snapshots were fetched when their pull began at cursor 0 (once per connection and
    /// server epoch).
    pub(super) bootstrapped: HashSet<FeedId>,
    /// The cursor each feed's pull (or inline apply) reached: a `changed` frame whose cursor is not
    /// above its feed's has nothing to pull.
    pub(super) cursors: HashMap<FeedId, u64>,
    /// The highest cursor a `changed` frame announced for each feed that was acted on: a frame that
    /// repeats it (or announces less) is not pulled for again, so a server that repeats itself costs
    /// nothing.
    announced: HashMap<FeedId, u64>,
    /// When each feed's snapshots were last fetched because items waited for them.
    pub(super) parked: HashMap<FeedId, Instant>,
}

impl<R> Link<R> {
    /// Whether a `changed` frame announces something this device has not reached or acted on yet (a feed
    /// it has not pulled yet may hold anything); a `true` answer is remembered as acted on.
    pub(super) fn announce(&mut self, feed: FeedId, cursor: u64) -> bool {
        if self
            .cursors
            .get(&feed)
            .is_some_and(|reached| cursor <= *reached)
        {
            return false;
        }
        let seen = self.announced.entry(feed).or_insert(0);
        if cursor <= *seen {
            return false;
        }
        *seen = cursor;
        true
    }

    /// The cursor a feed's pull (or an inline entry) reached.
    pub(super) fn note_cursor(&mut self, feed: FeedId, cursor: u64) {
        let reached = self.cursors.entry(feed).or_insert(cursor);
        *reached = (*reached).max(cursor);
    }

    /// What the driver knew of the server's feeds is stale (its data was reset).
    fn forget_progress(&mut self) {
        self.bootstrapped.clear();
        self.parked.clear();
        self.cursors.clear();
        self.announced.clear();
    }
}

/// The v2 sync loop.
pub struct SyncDriver;

impl SyncDriver {
    /// Subscribes to the host's events and starts the loop on `runtime` (the cycle in the
    /// [module documentation](super)). The first cycle runs at once when sync is configured. Dropping
    /// (or [`DriverHandle::stop`]ping) the handle ends it.
    #[must_use]
    pub fn spawn<R: SyncRemote>(
        host: Arc<dyn SyncHost>,
        remote_factory: RemoteFactory<R>,
        runtime: tokio::runtime::Handle,
    ) -> DriverHandle {
        let (events, receiver) = mpsc::unbounded_channel();
        let unsubscribe = host.subscribe(Box::new(move |event: &Event| {
            if matches!(event, Event::SyncRequested | Event::SyncChanged) {
                let _ = events.send(event.clone());
            }
        }));
        let (stop, stopped) = watch::channel(false);
        let realtime = RealtimeHandle::default();
        let driver = Driver {
            host,
            factory: remote_factory,
            link: None,
            invite_token: None,
            realtime: realtime.clone(),
        };
        let task = runtime.spawn(driver.run(receiver, stopped));
        tracing::debug!("sync driver (v2) started");
        DriverHandle::new(stop, task, unsubscribe, realtime)
    }
}

pub(super) struct Driver<R: SyncRemote> {
    pub(super) host: Arc<dyn SyncHost>,
    factory: RemoteFactory<R>,
    pub(super) link: Option<Link<R>>,
    /// A token taken from the core for an attempt that failed for a transient reason.
    invite_token: Option<String>,
    /// The slot the shell's [`RealtimeHandle`] sends presence through: the link's socket fills it.
    pub(super) realtime: RealtimeHandle,
}

impl<R: SyncRemote> Driver<R> {
    async fn run(
        mut self,
        mut events: mpsc::UnboundedReceiver<Event>,
        mut stop: watch::Receiver<bool>,
    ) {
        let mut plan = Plan::new();
        // A host that drops its subscription leaves the timer and the socket.
        let mut listening = true;
        self.reconfigure(&mut plan).await;
        loop {
            let wake = plan.next_wake();
            let expire = self.expire_at();
            tokio::select! {
                biased;
                _ = stop.changed() => break,
                event = events.recv(), if listening => {
                    let Some(mut event) = event else {
                        tracing::debug!("the host stopped sending events");
                        listening = false;
                        continue;
                    };
                    let mut requested = false;
                    let mut changed = false;
                    loop {
                        match event {
                            Event::SyncRequested => requested = true,
                            Event::SyncChanged => changed = true,
                            _ => {}
                        }
                        match events.try_recv() {
                            Ok(next) => event = next,
                            Err(_) => break,
                        }
                    }
                    if requested {
                        // The hint is read here, not in the core's event callback (which must not
                        // block); the burst counts from the first request.
                        let at = Instant::now();
                        let debounce = self.debounce().await;
                        plan.request(at, debounce);
                    }
                    if changed {
                        self.reconfigure(&mut plan).await;
                    }
                }
                event = next_event(&mut self.link) => match event {
                    Some(event) => self.on_realtime(event, &mut plan).await,
                    None => self.socket_ended(&mut plan),
                },
                () = sleep_until(expire) => self.expire().await,
                () = sleep_until(wake) => {
                    let kind = plan.take(Instant::now());
                    let outcome = self.cycle(kind).await;
                    plan.finished(outcome, Instant::now());
                }
            }
        }
        tracing::debug!("sync driver (v2) loop ended");
    }

    /// Re-reads the configuration: a new server drops the remote, no server idles.
    async fn reconfigure(&mut self, plan: &mut Plan) {
        let info = match self.host_call(|host| host.sync_info()).await {
            Ok(info) => info,
            Err(_) => {
                tracing::debug!("sync status unavailable: driver idle");
                plan.deactivate();
                return;
            }
        };
        let server = info.server_url.filter(|_| info.configured);
        let Some(url) = server else {
            if plan.active {
                tracing::info!("sync disabled");
            }
            self.link = None;
            self.invite_token = None;
            plan.deactivate();
            return;
        };
        let moved = self.link.as_ref().is_some_and(|link| link.url != url);
        if moved {
            tracing::info!("sync server changed: reconnecting");
            self.link = None;
            self.invite_token = None;
        }
        if !plan.active || moved {
            plan.activate(Instant::now());
        }
    }

    /// One cycle, its failure reported.
    async fn cycle(&mut self, kind: Kind) -> Outcome {
        match self.run_cycle(kind).await {
            Ok(outcome) => outcome,
            Err(error) => self.report(error).await,
        }
    }

    async fn run_cycle(&mut self, kind: Kind) -> Result<Outcome, NetError> {
        let info = self.host_call(|host| host.sync_info()).await?;
        let Some(url) = info.server_url.clone().filter(|_| info.configured) else {
            return Ok(Outcome::Idle);
        };
        let remote = self.connect(&url)?;
        let push = kind == Kind::Full || info.pending_entries > 0;
        tracing::debug!(?kind, push, "sync cycle");
        self.set_state(
            if push {
                SyncState::Pushing
            } else {
                SyncState::Pulling
            },
            None,
        )
        .await?;

        // The server, and whether it lost its data.
        let server = remote.server_info().await?;
        if server.protocol_version != PROTOCOL_VERSION {
            return Err(NetError::Sync(SyncError::Protocol(format!(
                "the server speaks sync protocol {}, this app speaks {PROTOCOL_VERSION}",
                server.protocol_version
            ))));
        }
        let epoch = server.server_epoch;
        if self
            .host_call(move |host| host.sync_remote_epoch(epoch))
            .await?
        {
            tracing::info!("the server's data was reset: publishing everything again");
            if let Some(link) = self.link.as_mut() {
                link.forget_progress();
            }
        }

        let Some(who) = self.host_call(|host| host.sync_identity()).await? else {
            return Err(NetError::Core(ApiError::invalid("kit", NO_KIT)));
        };
        // The device log is the account's, not a feed's: only a fork of the personal feed pauses
        // its publication (a fork somewhere else must not hold back a revocation).
        let personal = FeedId::from(who.0);
        let forked = if info.feeds.is_empty() {
            info.fork_detected
        } else {
            info.feeds
                .iter()
                .any(|feed| feed.feed_id == personal && feed.forked)
        };
        if self.sync_log(&remote, who.0, forked).await? == Step::Stop {
            return Ok(Outcome::Failed);
        }
        if remote.needs_authentication(who.0, who.1) {
            self.authenticate(&remote, who).await?;
        }
        if let Some(link) = self.link.as_mut() {
            link.session = Some((who, epoch));
        }
        // The realtime socket, with the session just opened; its subscription is sent with the feeds
        // below, before any of them is pulled.
        self.open_socket(&remote);
        self.recovery_blob(&remote, who).await?;

        let cx = Cx { who, epoch, push };
        let mut summary = Summary::default();
        // The personal feed first: a failure there is the account's own and ends the cycle.
        let feeds = self.host_call(|host| host.sync_feeds()).await?;
        self.subscribe_feeds(&feeds, false);
        for work in feeds.iter().filter(|work| work.kind == FeedKind::Personal) {
            self.feed_cycle(&remote, cx, work, &mut summary).await?;
        }
        // Then the co-members' logs the shared feeds are judged by.
        self.foreign_logs(&remote, &mut summary).await?;
        // Listed again: the personal pull may have taught the account document a new shared space.
        let feeds = self.host_call(|host| host.sync_feeds()).await?;
        self.subscribe_feeds(&feeds, false);
        for work in feeds.iter().filter(|work| work.kind == FeedKind::Shared) {
            if let Err(error) = self.feed_cycle(&remote, cx, work, &mut summary).await {
                let Some(message) = feed_failure(&error) else {
                    return Err(error);
                };
                tracing::info!(feed = %work.feed.short_hex(), "a shared feed's step stopped");
                if message == QUOTA_FULL {
                    summary.quota_full = true;
                } else {
                    summary.problems.push(message);
                }
            }
        }

        // View links (M13 slice 4): the uploads and deletions due, after the chunks they declare.
        self.work_links(&remote, cx, &mut summary).await?;

        if summary.membership_posted {
            // A feed this account just made, or was just admitted to, was not on the access list the
            // server checked the last subscription against.
            self.subscribe_feeds(&feeds, true);
        }

        let fork = self.host_call(|host| host.sync_info()).await?.fork_detected;
        if fork {
            if !forked {
                tracing::warn!("the server's history does not match this device's: uploads paused");
            }
            self.set_state(SyncState::Forked, None).await?;
            return Ok(Outcome::Synced);
        }
        if summary.quota_full {
            self.set_state(SyncState::Error, Some(QUOTA_FULL.to_owned()))
                .await?;
            return Ok(Outcome::Failed);
        }
        if let Some(message) = summary.problems.into_iter().next() {
            self.set_state(SyncState::Error, Some(message)).await?;
            return Ok(Outcome::Failed);
        }
        self.set_state(SyncState::Idle, None).await?;
        Ok(if summary.budget_used {
            Outcome::Busy
        } else {
            Outcome::Synced
        })
    }

    /// The remote for `url`, created on first use or after the address changed.
    fn connect(&mut self, url: &str) -> Result<Arc<R>, NetError> {
        if let Some(link) = &self.link
            && link.url == url
        {
            return Ok(Arc::clone(&link.remote));
        }
        let remote = (self.factory)(url)?;
        self.link = Some(Link {
            url: url.to_owned(),
            remote: Arc::clone(&remote),
            realtime: None,
            socket_backoff: Backoff::new(),
            session: None,
            bootstrapped: HashSet::new(),
            cursors: HashMap::new(),
            announced: HashMap::new(),
            parked: HashMap::new(),
        });
        Ok(remote)
    }

    /// The device log: learns the server's copy (how this device gets the other devices' keys; a log
    /// that does not extend ours is the core's `Crypto` error), publishes what the server lacks of ours,
    /// and creates the account when the server does not know it. While the personal feed is forked,
    /// only the download half runs.
    async fn sync_log(
        &mut self,
        remote: &Arc<R>,
        account: kb_types::AccountId,
        forked: bool,
    ) -> Result<Step, NetError> {
        let remote_log = match remote.fetch_log(account, None).await {
            Ok(entries) => entries,
            Err(SyncError::NotFound) if !forked => return self.create_account(remote).await,
            Err(error) => return Err(error.into()),
        };
        if !remote_log.is_empty() {
            let received = remote_log.clone();
            if self
                .host_call(move |host| host.sync_log_received(&received))
                .await?
            {
                tracing::debug!("device log entries received");
            }
        }
        if forked {
            return Ok(Step::Continue);
        }
        let (log, published) = self.host_call(|host| host.sync_account_log()).await?;
        // What the server holds of our log, from the start (the server refuses a tail that does not
        // continue its chain).
        let known = log
            .iter()
            .zip(&remote_log)
            .take_while(|(ours, theirs)| ours == theirs)
            .count();
        if known < log.len() {
            match remote.append_log(account, &log[known..]).await {
                Ok(()) => tracing::debug!(entries = log.len() - known, "device log published"),
                Err(SyncError::NotFound) => return self.create_account(remote).await,
                Err(error) => return Err(error.into()),
            }
        }
        let published = usize::try_from(published).unwrap_or(usize::MAX);
        if let Some(last) = log.len().checked_sub(1)
            && published < log.len()
        {
            let up_to_seq = last as u64;
            self.host_call(move |host| host.sync_log_published(up_to_seq))
                .await?;
        }
        Ok(Step::Continue)
    }

    /// Creates the account with the invite token of the last `SyncConfigure`. Without one (or with one
    /// the server refuses) the core records that a token is needed (`sync_account_unknown`) and the
    /// cycle stops there.
    async fn create_account(&mut self, remote: &Arc<R>) -> Result<Step, NetError> {
        let fresh = self.host_call(|host| host.sync_take_invite_token()).await?;
        let Some(token) = fresh.or_else(|| self.invite_token.take()) else {
            tracing::info!("the server does not know the account: an invite token is needed");
            self.host_call(|host| host.sync_account_unknown()).await?;
            return Ok(Step::Stop);
        };
        let (log, _) = self.host_call(|host| host.sync_account_log()).await?;
        match remote.create_account(&token, &log).await {
            Ok(()) => tracing::info!("account created on the server"),
            Err(SyncError::Unauthorized) => {
                // Ask for another token, saying why.
                self.host_call(|host| host.sync_account_unknown()).await?;
                self.set_state(SyncState::Error, Some(INVITE_TOKEN_REFUSED.to_owned()))
                    .await?;
                return Ok(Step::Stop);
            }
            Err(error @ SyncError::Unavailable(_)) => {
                // The core hands a token out once: keep it for the next attempt.
                self.invite_token = Some(token);
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(last) = log.len().checked_sub(1) {
            let up_to_seq = last as u64;
            self.host_call(move |host| host.sync_log_published(up_to_seq))
                .await?;
        }
        Ok(Step::Continue)
    }

    /// Opens a session, the core signing the challenge.
    pub(super) async fn authenticate(
        &self,
        remote: &Arc<R>,
        (account, device): Who,
    ) -> Result<(), NetError> {
        let host = Arc::clone(&self.host);
        let failure: Arc<Mutex<Option<ApiError>>> = Arc::default();
        let slot = Arc::clone(&failure);
        let signer: AuthSigner = Arc::new(move |message: &[u8]| match host.sync_sign(message) {
            Ok(signature) => Some(signature),
            Err(error) => {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                None
            }
        });
        match remote.authenticate(account, device, signer).await {
            Ok(()) => {
                tracing::debug!("session opened");
                Ok(())
            }
            Err(error) => {
                // The core could not sign: its reason, not the server's refusal.
                let core = failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                Err(core.map_or_else(|| error.into(), NetError::Core))
            }
        }
    }

    /// The recovery blob (ADR-0022 section 5): uploads the current one when the core says the server
    /// lacks it, then deletes the blobs of voided kits (one the server does not hold counts as
    /// deleted).
    async fn recovery_blob(&self, remote: &Arc<R>, who: Who) -> Result<(), NetError> {
        if let Some((id, blob)) = self.host_call(|host| host.sync_recovery_blob()).await? {
            with_session!(self, remote, who, remote.recovery_put(id, &blob))?;
            self.host_call(|host| host.sync_recovery_published())
                .await?;
            tracing::debug!("recovery blob published");
        }
        let obsolete = self.host_call(|host| host.sync_recovery_obsolete()).await?;
        for id in obsolete {
            match with_session!(self, remote, who, remote.recovery_delete(id)) {
                Ok(()) | Err(NetError::Sync(SyncError::NotFound)) => {}
                Err(error) => return Err(error),
            }
            self.host_call(move |host| host.sync_recovery_deleted(id))
                .await?;
            tracing::debug!("obsolete recovery blob deleted");
        }
        Ok(())
    }

    /// The co-members' device logs the core asks for (`logs_batch` needs no session). A log that does
    /// not verify (the core's `Crypto` error) is noted and the others are still handed over: one
    /// co-member must not hold back the account's own feeds.
    pub(super) async fn foreign_logs(
        &self,
        remote: &Arc<R>,
        summary: &mut Summary,
    ) -> Result<(), NetError> {
        let requests = self.host_call(|host| host.sync_needed_logs()).await?;
        if requests.is_empty() {
            return Ok(());
        }
        let chunks = remote.logs_batch(&requests).await?;
        for chunk in chunks {
            let (account, entries) = (chunk.account, chunk.entries);
            match self
                .host_call(move |host| host.sync_foreign_log_received(account, &entries))
                .await
            {
                Ok(added) => {
                    if added {
                        tracing::debug!("a co-member's device log was received");
                    }
                }
                Err(NetError::Core(error)) if error.code == ErrorCode::Crypto => {
                    tracing::warn!("a co-member's device log was refused");
                    summary.problems.push(error.message);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Reports why a cycle stopped.
    async fn report(&self, error: NetError) -> Outcome {
        let (state, message, outcome) = match error {
            NetError::Sync(SyncError::Unavailable(message)) | NetError::Transport(message) => {
                (SyncState::Offline, message, Outcome::Offline)
            }
            NetError::Core(error) if error.code == ErrorCode::Locked => {
                tracing::debug!("the workspace is locked: sync paused");
                return Outcome::Failed;
            }
            NetError::Core(error) => (SyncState::Error, error.message, Outcome::Failed),
            NetError::Refused(message) => (SyncState::Error, message, Outcome::Failed),
            // Another device has to re-admit it: no retry helps.
            NetError::Sync(SyncError::Idle) => {
                (SyncState::Error, IDLE_DEVICE.to_owned(), Outcome::Failed)
            }
            NetError::Sync(SyncError::Quota) => {
                (SyncState::Error, QUOTA_FULL.to_owned(), Outcome::Failed)
            }
            NetError::Sync(error) => (SyncState::Error, error.to_string(), Outcome::Failed),
        };
        tracing::info!(?state, "sync cycle stopped");
        if self.set_state(state, Some(message)).await.is_err() {
            tracing::debug!("the sync state could not be recorded");
        }
        outcome
    }

    pub(super) async fn set_state(
        &self,
        state: SyncState,
        error: Option<String>,
    ) -> Result<(), NetError> {
        self.host_call(move |host| host.sync_set_state(state, error))
            .await
    }

    /// Runs a core call on the blocking pool (it waits for the shell's lock).
    pub(super) async fn host_call<T, F>(&self, call: F) -> Result<T, NetError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn SyncHost) -> Result<T, ApiError> + Send + 'static,
    {
        let host = Arc::clone(&self.host);
        match tokio::task::spawn_blocking(move || call(host.as_ref())).await {
            Ok(result) => result.map_err(NetError::Core),
            Err(_) => Err(NetError::Core(ApiError::internal(
                "the core stopped during a sync step",
            ))),
        }
    }
}

/// The message to show when a failure concerns one shared feed only (the cycle goes on with the other
/// feeds), `None` when it ends the whole cycle: the server cannot be reached, the session or the
/// device is refused, or the core failed for a reason of its own.
pub(super) fn feed_failure(error: &NetError) -> Option<String> {
    match error {
        NetError::Sync(SyncError::Quota) => Some(QUOTA_FULL.to_owned()),
        NetError::Sync(SyncError::Unavailable(_) | SyncError::Unauthorized | SyncError::Idle) => {
            None
        }
        NetError::Sync(other) => Some(other.to_string()),
        // A forged item or a key that does not match: that feed's, not the account's.
        NetError::Core(error) if error.code == ErrorCode::Crypto => Some(error.message.clone()),
        NetError::Core(_) | NetError::Transport(_) | NetError::Refused(_) => None,
    }
}
