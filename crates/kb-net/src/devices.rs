//! The network halves of device pairing and kit recovery (ADR-0022,
//! `docs/development/milestone-07.md`, "kb-net"). The core runs both state machines
//! (sans-IO); these tasks move bytes between it and the server.
//!
//! **Pairing** ([`PairingTask`]), either role, over a [`PairingHost`]:
//!
//! - joiner: `pairing_outbox` holds the clear offer as message 0 before any mailbox
//!   exists; `pair_create(offer)` opens the mailbox, then `pairing_mailbox_opened` and
//!   `pairing_posted(0)`; the task then polls from message 1;
//! - sponsor: the core decoded the typed code, so `pairing_mailbox_id` names the mailbox;
//!   the task reads from message 0 (the offer) and tells the core the expiry with
//!   `pairing_mailbox_opened` on the first answer;
//! - both, until `pairing_info().state` is `Done`, `Failed` or `Cancelled`: post
//!   whatever `pairing_outbox` holds (then `pairing_posted`), long-poll `pair_messages`
//!   (`PAIRING_POLL_SECONDS`) and hand every message of the other role to
//!   `pairing_receive`; a `PairingChanged` event (the user confirmed or cancelled) wakes
//!   the task at once;
//! - the sponsor publishes the unpublished tail of its device log (the joiner's
//!   `DeviceAdded`, through `fetch_log`, `append_log` and `sync_log_published`) before it
//!   posts anything, so the log is on the server before the bundle leaves; it closes the
//!   mailbox when it is done. Either side closes it after a failure or a cancellation.
//!
//! An unknown or expired mailbox ends the session with [`CODE_EXPIRED`]; a mailbox the
//! other side closed with [`PEER_LEFT`]; other network failures are retried (three
//! attempts, 1 s then 2 s apart) and then end it with their message.
//!
//! **Recovery** ([`RecoveryTask`]) over a [`RecoveryHost`]: `recovery_get(recovery_id)`
//! (nothing stored: [`KIT_UNKNOWN`]), `recovery_blob_received`, `fetch_log` of the
//! account `recovery_account_id` then names, `recovery_log_received` (answers this device's
//! `DeviceAdded`), `append_log` of that entry, `recovery_entry_published`. The sync
//! driver bootstraps the device afterwards.
//!
//! [`DeviceTasks`] starts a pairing task on `PairingRequested` and a recovery task on
//! `RecoveryRequested` (a new request replaces a task still running). Dropping a
//! [`TaskHandle`] stops its task. Every host call runs on tokio's blocking pool.
//! Nothing here logs a message's bytes, the kit, a root key or a SAS; mailbox ids may be
//! logged (they expire in ten minutes) but are not.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use kb_sync::{PAIRING_ROLE_JOINER, PAIRING_ROLE_SPONSOR, SyncError};
use kb_types::devices::{MAILBOX_ID_LEN, PAIRING_POLL_SECONDS};
use kb_types::{
    AccountId, ApiError, DeviceId, ErrorCode, Event, PairingInfo, PairingRole, PairingState,
    RecoveryInfo, RecoveryState,
};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::driver::RemoteFactory;
use crate::v2::{RemoteFuture, SyncRemote};

/// The session ended because the mailbox is unknown or expired.
pub const CODE_EXPIRED: &str = "the code expired";
/// The session ended because the other device closed the mailbox.
pub const PEER_LEFT: &str = "the other device gave up";
/// The recovery ended because the server holds no blob for the kit.
pub const KIT_UNKNOWN: &str = "this server does not know that kit";
/// The sponsor's account is not on the server yet.
pub const ACCOUNT_NOT_ON_SERVER: &str =
    "this device's account is not on the server yet; let it sync first";

/// Attempts of a remote call that fails for a network reason.
const ATTEMPTS: u32 = 3;
/// Delay before the second attempt (doubled for the third).
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// The least time between two polls that brought nothing (a server that does not hold
/// the request, such as the reference remote, would otherwise be asked in a loop).
const QUIET_POLL: Duration = Duration::from_secs(1);

/// What the pairing task needs from the core (the shell's `AppState`: each method one
/// call under the single-writer lock).
pub trait PairingHost: Send + Sync + 'static {
    /// The session as the UI shows it (role, state, server).
    fn pairing_info(&self) -> Result<PairingInfo, ApiError>;
    /// The mailbox of the session: the sponsor's from the typed code, the joiner's once
    /// `pairing_mailbox_opened` told the core.
    fn pairing_mailbox_id(&self) -> Result<Option<[u8; MAILBOX_ID_LEN]>, ApiError>;
    /// The mailbox is open (joiner) or found (sponsor), and when it expires (Unix ms).
    fn pairing_mailbox_opened(
        &self,
        mailbox_id: [u8; MAILBOX_ID_LEN],
        expires_at: i64,
    ) -> Result<(), ApiError>;
    /// Messages to post, by the core's sequence number (the joiner's offer is 0).
    fn pairing_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError>;
    /// The message `seq` of the outbox was posted.
    fn pairing_posted(&self, seq: u32) -> Result<(), ApiError>;
    /// A message of the other side (`role` is the sender's), with its mailbox sequence.
    fn pairing_receive(&self, seq: u32, role: PairingRole, bytes: &[u8]) -> Result<(), ApiError>;
    /// The task gave up; `message` is shown to the user.
    fn pairing_failed(&self, message: &str) -> Result<(), ApiError>;
    /// The account and this device, for the sponsor's log publication.
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
    /// The device log and how many entries, from the start, are published.
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
    /// The server holds every entry with a sequence `<= up_to_seq`.
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError>;
    /// Core events (the tasks react to `PairingRequested`, `RecoveryRequested` and
    /// `PairingChanged`); the callback must not block. The closure unsubscribes.
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send>;
}

/// What the recovery task needs from the core.
pub trait RecoveryHost: Send + Sync + 'static {
    /// The recovery as the UI shows it (state, server).
    fn recovery_info(&self) -> Result<RecoveryInfo, ApiError>;
    /// The lookup id of the recovery blob, derived from the kit.
    fn recovery_id(&self) -> Result<Option<[u8; 32]>, ApiError>;
    /// The blob fetched from the server.
    fn recovery_blob_received(&self, bytes: &[u8]) -> Result<(), ApiError>;
    /// The account's device log; answers this device's `DeviceAdded` to publish.
    fn recovery_log_received(&self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError>;
    /// The server took the entry.
    fn recovery_entry_published(&self) -> Result<(), ApiError>;
    /// The task gave up; `message` is shown to the user.
    fn recovery_failed(&self, message: &str) -> Result<(), ApiError>;
    /// The account the blob belongs to (known once the blob is in; a fresh device has
    /// no device log yet, so `sync_identity` cannot answer).
    fn recovery_account_id(&self) -> Result<Option<AccountId>, ApiError>;
}

/// A spawned task. Dropping the handle stops it at its next await point and ends its
/// event subscription at once.
pub struct TaskHandle {
    task: Option<JoinHandle<()>>,
    subscription: Option<Subscription>,
}

impl TaskHandle {
    /// A handle for the task of another module of this crate (the invite task).
    pub(crate) fn new(task: JoinHandle<()>, subscription: Option<Subscription>) -> Self {
        Self {
            task: Some(task),
            subscription,
        }
    }

    /// Stops the task; never waits for it.
    pub fn stop(mut self) {
        self.abort();
    }

    /// Whether the task is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    fn abort(&mut self) {
        if let Some(subscription) = self.subscription.take() {
            subscription.end();
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.abort();
    }
}

/// A task's event subscription, ended by whichever comes first: the task finishing (its
/// [`Ends`] guard) or its handle being dropped.
#[derive(Clone)]
pub(crate) struct Subscription(Arc<Mutex<Option<Unsubscribe>>>);

/// What `subscribe` answers.
type Unsubscribe = Box<dyn FnOnce() + Send>;

impl Subscription {
    pub(crate) fn new(unsubscribe: Unsubscribe) -> Self {
        Self(Arc::new(Mutex::new(Some(unsubscribe))))
    }

    pub(crate) fn end(&self) {
        let unsubscribe = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
    }
}

/// Ends the subscription when the task's future is dropped.
pub(crate) struct Ends(pub(crate) Subscription);

impl Drop for Ends {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// Runs a core call on the blocking pool (it waits for the shell's lock).
pub(crate) async fn host_call<H, T, F>(host: &Arc<H>, call: F) -> Result<T, Failure>
where
    H: ?Sized + Send + Sync + 'static,
    T: Send + 'static,
    F: FnOnce(&H) -> Result<T, ApiError> + Send + 'static,
{
    let host = Arc::clone(host);
    match tokio::task::spawn_blocking(move || call(host.as_ref())).await {
        Ok(result) => result.map_err(Failure::Core),
        Err(_) => Err(Failure::Core(ApiError::internal(
            "the core stopped during a device task",
        ))),
    }
}

/// A remote call, retried when the network failed (three attempts in all).
pub(crate) async fn retried<'a, T>(
    mut call: impl FnMut() -> RemoteFuture<'a, T>,
) -> Result<T, SyncError> {
    let mut delay = RETRY_DELAY;
    let mut attempt = 1;
    loop {
        match call().await {
            Err(SyncError::Unavailable(_)) if attempt < ATTEMPTS => {
                tokio::time::sleep(delay).await;
                delay *= 2;
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Why a task stopped.
#[derive(Debug)]
pub(crate) enum Failure {
    /// A core call failed.
    Core(ApiError),
    /// A remote call failed (after the retries).
    Remote(SyncError),
    /// What the user is told.
    Message(String),
}

impl Failure {
    pub(crate) fn message(&self) -> String {
        match self {
            Failure::Core(error) => error.message.clone(),
            Failure::Remote(SyncError::Unavailable(message)) | Failure::Message(message) => {
                message.clone()
            }
            Failure::Remote(error) => error.to_string(),
        }
    }

    pub(crate) fn locked(&self) -> bool {
        matches!(self, Failure::Core(error) if error.code == ErrorCode::Locked)
    }
}

/// A failure of a mailbox call: unknown or expired is [`CODE_EXPIRED`], closed or full is
/// [`PEER_LEFT`].
pub(crate) fn mailbox_failure(error: SyncError) -> Failure {
    match error {
        SyncError::NotFound => Failure::Message(CODE_EXPIRED.to_owned()),
        SyncError::Conflict(_) => Failure::Message(PEER_LEFT.to_owned()),
        other => Failure::Remote(other),
    }
}

// ----- pairing --------------------------------------------------------------------------------------

/// The pairing loop of either role.
pub struct PairingTask;

impl PairingTask {
    /// Starts the task for the core's current session. It ends with the session.
    #[must_use]
    pub fn spawn<R: SyncRemote>(
        host: Arc<dyn PairingHost>,
        remote_factory: RemoteFactory<R>,
        runtime: &tokio::runtime::Handle,
    ) -> TaskHandle {
        let changed = Arc::new(Notify::new());
        let wake = Arc::clone(&changed);
        let unsubscribe = host.subscribe(Box::new(move |event: &Event| {
            if matches!(event, Event::PairingChanged) {
                wake.notify_one();
            }
        }));
        let subscription = Subscription::new(unsubscribe);
        let ends = Ends(subscription.clone());
        let task = runtime.spawn(async move {
            let _ends = ends;
            run_pairing(host, remote_factory, changed).await;
        });
        TaskHandle {
            task: Some(task),
            subscription: Some(subscription),
        }
    }
}

fn over(state: PairingState) -> bool {
    matches!(
        state,
        PairingState::Idle | PairingState::Done | PairingState::Failed | PairingState::Cancelled
    )
}

fn role_byte(role: PairingRole) -> u8 {
    match role {
        PairingRole::Joiner => PAIRING_ROLE_JOINER,
        PairingRole::Sponsor => PAIRING_ROLE_SPONSOR,
    }
}

fn peer(role: PairingRole) -> PairingRole {
    match role {
        PairingRole::Joiner => PairingRole::Sponsor,
        PairingRole::Sponsor => PairingRole::Joiner,
    }
}

async fn run_pairing<R: SyncRemote>(
    host: Arc<dyn PairingHost>,
    factory: RemoteFactory<R>,
    changed: Arc<Notify>,
) {
    let Ok(info) = host_call(&host, |host| host.pairing_info()).await else {
        return;
    };
    if over(info.state) {
        return;
    }
    let remote = match info.server_url.as_deref().map(|url| factory(url)) {
        Some(Ok(remote)) => remote,
        Some(Err(error)) => {
            fail_pairing(&host, &error.to_string()).await;
            return;
        }
        None => {
            fail_pairing(&host, "no server for the pairing").await;
            return;
        }
    };
    let mut session = Pairing {
        host: Arc::clone(&host),
        remote,
        role: info.role,
        mailbox: None,
        changed,
    };
    let outcome = session.run().await;
    let close = match &outcome {
        // The sponsor's bundle is posted: nothing more to say.
        Ok(PairingState::Done) => info.role == PairingRole::Sponsor,
        Ok(_) => true,
        Err(failure) if failure.locked() => {
            tracing::debug!("the workspace is locked: pairing task stopped");
            return;
        }
        Err(failure) => {
            tracing::info!(role = ?info.role, "pairing failed");
            // The core may have ended the session itself (a message that did not open).
            let ended = host_call(&host, |host| host.pairing_info())
                .await
                .is_ok_and(|info| over(info.state));
            if !ended {
                fail_pairing(&host, &failure.message()).await;
            }
            !matches!(failure, Failure::Message(text) if text == CODE_EXPIRED)
        }
    };
    if close
        && let Some(id) = session.mailbox
        && session.remote.pair_close(id).await.is_err()
    {
        tracing::debug!("the pairing mailbox could not be closed");
    }
    tracing::debug!(role = ?info.role, "pairing task ended");
}

async fn fail_pairing(host: &Arc<dyn PairingHost>, message: &str) {
    let message = message.to_owned();
    if host_call(host, move |host| host.pairing_failed(&message))
        .await
        .is_err()
    {
        tracing::debug!("the pairing failure could not be recorded");
    }
}

struct Pairing<R> {
    host: Arc<dyn PairingHost>,
    remote: Arc<R>,
    role: PairingRole,
    mailbox: Option<[u8; MAILBOX_ID_LEN]>,
    changed: Arc<Notify>,
}

impl<R: SyncRemote> Pairing<R> {
    /// Runs the session to its end; answers the final state.
    async fn run(&mut self) -> Result<PairingState, Failure> {
        let (id, mut next, mut announced) = self.open().await?;
        let mine = role_byte(self.role);
        let theirs = peer(self.role);
        loop {
            // Armed before the state is read: a confirmation in between still wakes us.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();

            let info = host_call(&self.host, |host| host.pairing_info()).await?;
            if over(info.state) {
                return Ok(info.state);
            }
            let outbox = host_call(&self.host, |host| host.pairing_outbox()).await?;
            if !outbox.is_empty() {
                if self.role == PairingRole::Sponsor {
                    self.publish_log().await?;
                }
                for (seq, bytes) in outbox {
                    let remote = &self.remote;
                    retried(|| remote.pair_post(id, mine, &bytes))
                        .await
                        .map_err(mailbox_failure)?;
                    host_call(&self.host, move |host| host.pairing_posted(seq)).await?;
                }
                continue;
            }

            let asked = Instant::now();
            let remote = &self.remote;
            tokio::select! {
                page = retried(|| remote.pair_messages(id, next, PAIRING_POLL_SECONDS)) => {
                    let page = page.map_err(mailbox_failure)?;
                    if !announced {
                        let expires_at = page.expires_at;
                        host_call(&self.host, move |host| host.pairing_mailbox_opened(id, expires_at))
                            .await?;
                        announced = true;
                    }
                    let mut fresh = 0;
                    for message in page.messages {
                        if message.seq < next {
                            continue;
                        }
                        next = message.seq.saturating_add(1);
                        if message.role == mine {
                            continue;
                        }
                        fresh += 1;
                        let (seq, bytes) = (message.seq, message.bytes);
                        host_call(&self.host, move |host| host.pairing_receive(seq, theirs, &bytes))
                            .await?;
                    }
                    if fresh == 0 {
                        if page.closed {
                            return Err(Failure::Message(PEER_LEFT.to_owned()));
                        }
                        // A server that answered at once with nothing: do not ask again
                        // straight away (the user can still wake us).
                        let quiet = QUIET_POLL.saturating_sub(asked.elapsed());
                        if !quiet.is_zero() {
                            tokio::select! {
                                () = tokio::time::sleep(quiet) => {}
                                () = changed.as_mut() => {}
                            }
                        }
                    }
                }
                () = changed.as_mut() => {}
            }
        }
    }

    /// The mailbox, the first message to read and whether the core knows the expiry.
    async fn open(&mut self) -> Result<([u8; MAILBOX_ID_LEN], u32, bool), Failure> {
        let known = host_call(&self.host, |host| host.pairing_mailbox_id()).await?;
        match (self.role, known) {
            // A joiner whose mailbox is open already (the task was restarted).
            (PairingRole::Joiner, Some(id)) => {
                self.mailbox = Some(id);
                Ok((id, 1, true))
            }
            (PairingRole::Joiner, None) => {
                let outbox = host_call(&self.host, |host| host.pairing_outbox()).await?;
                let Some((_, offer)) = outbox.into_iter().find(|(seq, _)| *seq == 0) else {
                    return Err(Failure::Core(ApiError::internal(
                        "the pairing session has no offer to post",
                    )));
                };
                let remote = &self.remote;
                let (id, expires_at) = retried(|| remote.pair_create(&offer))
                    .await
                    .map_err(Failure::Remote)?;
                self.mailbox = Some(id);
                host_call(&self.host, move |host| {
                    host.pairing_mailbox_opened(id, expires_at)
                })
                .await?;
                host_call(&self.host, |host| host.pairing_posted(0)).await?;
                tracing::debug!("pairing mailbox opened");
                Ok((id, 1, true))
            }
            (PairingRole::Sponsor, Some(id)) => {
                self.mailbox = Some(id);
                Ok((id, 0, false))
            }
            (PairingRole::Sponsor, None) => Err(Failure::Core(ApiError::internal(
                "the pairing session has no mailbox",
            ))),
        }
    }

    /// The sponsor's unpublished log tail (the joiner's `DeviceAdded`) goes to the server
    /// before anything else is posted: the joiner must find itself in the log when it
    /// installs the bundle.
    async fn publish_log(&self) -> Result<(), Failure> {
        let (log, published) = host_call(&self.host, |host| host.sync_account_log()).await?;
        let published = usize::try_from(published).map_or(log.len(), |n| n.min(log.len()));
        let Some(last) = log.len().checked_sub(1) else {
            return Ok(());
        };
        if published == log.len() {
            return Ok(());
        }
        let Some((account, _)) = host_call(&self.host, |host| host.sync_identity()).await? else {
            return Err(Failure::Message(
                "create your recovery kit before adding a device".to_owned(),
            ));
        };
        let remote = &self.remote;
        let held =
            retried(|| remote.fetch_log(account, None))
                .await
                .map_err(|error| match error {
                    SyncError::NotFound => Failure::Message(ACCOUNT_NOT_ON_SERVER.to_owned()),
                    other => Failure::Remote(other),
                })?;
        let known = log
            .iter()
            .zip(&held)
            .take_while(|(ours, theirs)| ours == theirs)
            .count();
        if known < log.len() {
            let tail = &log[known..];
            retried(|| remote.append_log(account, tail))
                .await
                .map_err(Failure::Remote)?;
        }
        let up_to_seq = last as u64;
        host_call(&self.host, move |host| host.sync_log_published(up_to_seq)).await?;
        tracing::debug!(
            entries = log.len() - known,
            "device log published for the new device"
        );
        Ok(())
    }
}

// ----- recovery ----------------------------------------------------------------------------------------

/// The recovery through the kit.
pub struct RecoveryTask;

impl RecoveryTask {
    /// Starts the task for the core's current recovery. It ends with it.
    #[must_use]
    pub fn spawn<R: SyncRemote>(
        host: Arc<dyn RecoveryHost>,
        remote_factory: RemoteFactory<R>,
        runtime: &tokio::runtime::Handle,
    ) -> TaskHandle {
        let task = runtime.spawn(run_recovery(host, remote_factory));
        TaskHandle {
            task: Some(task),
            subscription: None,
        }
    }
}

fn recovering(state: RecoveryState) -> bool {
    matches!(state, RecoveryState::Fetching | RecoveryState::Joining)
}

async fn run_recovery<R: SyncRemote>(host: Arc<dyn RecoveryHost>, factory: RemoteFactory<R>) {
    let Ok(info) = host_call(&host, |host| host.recovery_info()).await else {
        return;
    };
    if !recovering(info.state) {
        return;
    }
    let outcome = match info.server_url.as_deref().map(|url| factory(url)) {
        Some(Ok(remote)) => recover(&host, &remote).await,
        Some(Err(error)) => Err(Failure::Message(error.to_string())),
        None => Err(Failure::Message("no server for the recovery".to_owned())),
    };
    match outcome {
        Ok(()) => tracing::info!("recovered through the kit"),
        Err(failure) if failure.locked() => {
            tracing::debug!("the workspace is locked: recovery task stopped");
        }
        Err(failure) => {
            tracing::info!("recovery failed");
            let ended = host_call(&host, |host| host.recovery_info())
                .await
                .is_ok_and(|info| !recovering(info.state));
            if !ended {
                let message = failure.message();
                if host_call(&host, move |host| host.recovery_failed(&message))
                    .await
                    .is_err()
                {
                    tracing::debug!("the recovery failure could not be recorded");
                }
            }
        }
    }
}

async fn recover<R: SyncRemote>(
    host: &Arc<dyn RecoveryHost>,
    remote: &Arc<R>,
) -> Result<(), Failure> {
    let Some(id) = host_call(host, |host| host.recovery_id()).await? else {
        return Err(Failure::Core(ApiError::internal(
            "the recovery has no kit to look up",
        )));
    };
    let blob = retried(|| remote.recovery_get(id))
        .await
        .map_err(Failure::Remote)?
        .ok_or_else(|| Failure::Message(KIT_UNKNOWN.to_owned()))?;
    host_call(host, move |host| host.recovery_blob_received(&blob)).await?;
    let Some(account) = host_call(host, |host| host.recovery_account_id()).await? else {
        return Err(Failure::Core(ApiError::internal(
            "the recovery blob named no account",
        )));
    };
    let log = retried(|| remote.fetch_log(account, None))
        .await
        .map_err(|error| match error {
            SyncError::NotFound => Failure::Message(KIT_UNKNOWN.to_owned()),
            other => Failure::Remote(other),
        })?;
    let entry = host_call(host, move |host| host.recovery_log_received(&log)).await?;
    let entries = [entry];
    retried(|| remote.append_log(account, &entries))
        .await
        .map_err(Failure::Remote)?;
    host_call(host, |host| host.recovery_entry_published()).await
}

// ----- starting them ------------------------------------------------------------------------------------

/// Starts the pairing and recovery tasks when the core asks for them.
pub struct DeviceTasks;

/// What [`DeviceTasks`] serves: the core's pairing and recovery halves.
pub trait DeviceHost: PairingHost + RecoveryHost {}

impl<T: PairingHost + RecoveryHost> DeviceHost for T {}

impl DeviceTasks {
    /// Listens for `PairingRequested` and `RecoveryRequested` and runs the matching
    /// task; a new request replaces a task of the same kind that still runs. Dropping
    /// the handle stops the listener and both tasks.
    #[must_use]
    pub fn spawn<H: DeviceHost, R: SyncRemote>(
        host: Arc<H>,
        remote_factory: RemoteFactory<R>,
        runtime: tokio::runtime::Handle,
    ) -> TaskHandle {
        let (requests, mut received) = mpsc::unbounded_channel();
        let unsubscribe = PairingHost::subscribe(
            host.as_ref(),
            Box::new(move |event: &Event| {
                if matches!(event, Event::PairingRequested | Event::RecoveryRequested) {
                    let _ = requests.send(event.clone());
                }
            }),
        );
        let subscription = Subscription::new(unsubscribe);
        let ends = Ends(subscription.clone());
        let spawner = runtime.clone();
        let task = runtime.spawn(async move {
            let _ends = ends;
            let mut pairing: Option<TaskHandle> = None;
            let mut recovery: Option<TaskHandle> = None;
            while let Some(event) = received.recv().await {
                match event {
                    Event::PairingRequested => {
                        tracing::debug!("pairing task starting");
                        let host: Arc<dyn PairingHost> = Arc::clone(&host) as Arc<dyn PairingHost>;
                        pairing = Some(PairingTask::spawn(
                            host,
                            Arc::clone(&remote_factory),
                            &spawner,
                        ));
                    }
                    Event::RecoveryRequested => {
                        tracing::debug!("recovery task starting");
                        let host: Arc<dyn RecoveryHost> =
                            Arc::clone(&host) as Arc<dyn RecoveryHost>;
                        recovery = Some(RecoveryTask::spawn(
                            host,
                            Arc::clone(&remote_factory),
                            &spawner,
                        ));
                    }
                    _ => {}
                }
            }
            drop((pairing, recovery));
        });
        TaskHandle {
            task: Some(task),
            subscription: Some(subscription),
        }
    }
}

#[cfg(test)]
mod tests;
