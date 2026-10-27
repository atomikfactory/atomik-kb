//! The network half of invites (M13 slice 1, ADR-0046, `docs/development/milestone-13.md` section 1.6):
//! [`InviteTask`], both sides and both flows, over an [`InviteHost`] (the shell's `AppState`: each
//! method one call under the single-writer lock, one to one with `kb_core::Core`'s `invite_*`
//! methods in `crates/kb-core/src/invites.rs`), built on the pairing task of `devices.rs`.
//!
//! **Code flow.** The inviter's task opens the mailbox (`pair_create` with the offer from
//! `invite_outbox`, then `invite_mailbox_opened` and `invite_posted(0)`); the invitee's task reads from
//! message 0 (`invite_mailbox_id` from the typed code). Both, until `invite_info().state` is `Done`,
//! `Failed` or `Cancelled`: post what `invite_outbox` holds, long-poll `pair_messages`
//! (`PAIRING_POLL_SECONDS`) and hand every message of the other side to `invite_receive`; an
//! `InviteChanged` event wakes the task. The inviter's `Invite` entry goes to the server by
//! compare-and-swap (`invite_membership_pending`, `membership_append`, `invite_membership_posted`)
//! **before** the bundle is posted (the core puts the bundle in the outbox once the entry is taken, by
//! this task or by the sync driver); the account's log is on the server first. Once the bundle is
//! posted the inviter's task closes the mailbox and ends: the session becomes `Done` when the sync
//! driver folds the invitee's `Join`. The invitee then finishes against the server: the share signup
//! when the account is not there (`invite_signup`, `account_create_share`, then `sync_log_published`),
//! authentication, `invite_open_request` and `invite_open`, `invite_opened` (looped while the core still
//! needs a signer's device log: `sync_needed_logs`, `logs_batch`, `sync_foreign_log_received`), then the
//! `Join` (`invite_join`, `membership_append`, `invite_joined`).
//!
//! **Link flow** (invitee only): `invite_open_request` and `invite_open` as above, `invite_opened`
//! (the state becomes `Previewing`), and after the user accepted (`InviteConfirm`, state `Joining`) the
//! signup, authentication, the `Join` and `invite_joined`. A preview nobody answers ends as `Failed`
//! after half an hour.
//!
//! A cancellation or a failure the core ended the session with leaves an abort message in the outbox
//! (mailbox sequence: the side's last + 2): the task posts it and closes the mailbox. Timeouts and an
//! unknown or expired mailbox end the session as `Failed` ("the code expired"); every other network
//! failure is retried three times (1 s, then 2 s apart) before it ends it with its message. Every
//! 404 of the invite routes reads the same ("this invite is invalid, expired or already used"). Nothing
//! here logs a message's bytes, the code, the link, the SAS or a name.
//!
//! [`InviteTasks`] starts a task on each `InviteRequested` (a new request replaces a task still
//! running). Every host call runs on tokio's blocking pool.

use std::sync::Arc;
use std::time::Duration;

use kb_sync::SyncError;
use kb_sync::v2::{
    InviteOpenRequest, InviteOpened, InviteSignup, LogRequest, MembershipHead, PAIR_ROLE_OPENER,
    PAIR_ROLE_TYPIST, PendingMembership, peek_membership,
};
use kb_types::devices::PAIRING_POLL_SECONDS;
use kb_types::sync::PROTOCOL_VERSION;
use kb_types::{
    AccountId, ApiError, DeviceId, Event, FeedId, InviteFlow, InviteSessionInfo,
    InviteSessionState, InviteSide,
};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

use crate::devices::{
    CODE_EXPIRED, Ends, Failure, Subscription, TaskHandle, host_call, mailbox_failure, retried,
};
use crate::driver::{IDLE_DEVICE, RemoteFactory};
use crate::http::AuthSigner;
use crate::v2::SyncRemote;

/// What the user is told when the server does not know the invite, or it was used, revoked or
/// expired (one message for every 404, so that nobody can tell the states apart).
pub const INVALID_INVITE: &str = "this invite is invalid, expired or already used";
/// What the user is told when the server will not create an account from an invitation.
pub const SIGNUP_REFUSED: &str =
    "the server will not create an account from this invitation: its limit was reached";
/// What the user is told when the space's membership changed while the invitee was joining.
pub const SPACE_CHANGED: &str =
    "the space changed while you were joining; ask for the invitation again";
/// What the user is told when the inviter can no longer write the invite.
pub const CANNOT_INVITE: &str = "you can no longer invite people to this space";
/// What the user is told when the invite could not be put on the server for lack of luck.
pub const INVITE_NOT_WRITTEN: &str = "the invite could not be written; try again";
/// What the user is told when a signer's device log never came.
pub const NOT_VERIFIED: &str = "this invite could not be verified";
/// What the user is told when a link's preview was never answered.
pub const PREVIEW_TIMEOUT_MESSAGE: &str = "the invitation was not accepted in time";
/// What the user is told when the server speaks another protocol.
pub const WRONG_PROTOCOL: &str = "this server speaks another sync protocol";

/// How long a link's preview waits for the user.
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The least time between two polls that brought nothing (a remote that answers at once, such as the
/// reference one, would otherwise be asked in a loop).
const QUIET_POLL: Duration = Duration::from_secs(1);
/// Lost compare-and-swaps in a row after which the inviter gives up writing the `Invite`.
const MAX_CONFLICTS: u32 = 30;
/// Times the invitee fetches signers' logs while the core verifies the opened invite.
const MAX_LOG_ROUNDS: usize = 8;

/// What the invite task needs from the core.
pub trait InviteHost: Send + Sync + 'static {
    /// The session as the dialogs show it.
    fn invite_info(&self) -> Result<InviteSessionInfo, ApiError>;
    /// The mailbox of the session.
    fn invite_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError>;
    /// The mailbox is open (inviter) or found (invitee), and when it expires (Unix ms).
    fn invite_mailbox_opened(&self, mailbox_id: [u8; 6], expires_at: i64) -> Result<(), ApiError>;
    /// Messages to post, by mailbox sequence (the inviter's offer is 0).
    fn invite_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError>;
    /// The message `seq` of the outbox was posted.
    fn invite_posted(&self, seq: u32) -> Result<(), ApiError>;
    /// A message of the other side with its mailbox sequence.
    fn invite_receive(&self, seq: u32, from: InviteSide, bytes: &[u8]) -> Result<(), ApiError>;
    /// The task gave up; `message` is shown to the user.
    fn invite_failed(&self, message: &str) -> Result<(), ApiError>;
    /// The inviter's membership entries that must be on the server before the bundle leaves.
    fn invite_membership_pending(&self) -> Result<Option<PendingMembership>, ApiError>;
    /// The server took them; its membership head.
    fn invite_membership_posted(&self, head: MembershipHead) -> Result<(), ApiError>;
    /// The invitee's account signup claim, when the account is not on the server.
    fn invite_signup(&self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError>;
    /// The signed request to open the invite.
    fn invite_open_request(
        &self,
        server_epoch: [u8; 16],
    ) -> Result<Option<InviteOpenRequest>, ApiError>;
    /// The server's answer to the open request. The core verifies it; while a signer's device log is
    /// missing it asks for it (`sync_needed_logs`) and leaves the session as it is.
    fn invite_opened(&self, opened: &InviteOpened) -> Result<(), ApiError>;
    /// The `Join` entry to post once the user accepted.
    fn invite_join(&self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError>;
    /// The server took the `Join`; its membership head.
    fn invite_joined(&self, head: MembershipHead) -> Result<(), ApiError>;
    /// The account's identity for the signup and authentication: the account id and this device.
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
    /// Signs an authentication message with the device signing key.
    fn sync_sign(&self, message: &[u8]) -> Result<kb_crypto::Signature, ApiError>;
    /// The account's device log and how many entries, from the start, are published.
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
    /// The server holds every log entry with a sequence number `<= up_to_seq`.
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError>;
    /// The co-members' device logs the core needs (the signers of an opened invite among them).
    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError>;
    /// A co-member's device log as the server holds it.
    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError>;
    /// Core events (the task reacts to `InviteRequested` and `InviteChanged`); the callback must not
    /// block. The closure unsubscribes.
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send>;
}

/// The invite loop of either side and either flow.
pub struct InviteTask;

impl InviteTask {
    /// Starts the task for the core's current session. It ends with the session. Dropping the handle
    /// stops it.
    #[must_use]
    pub fn spawn<R: SyncRemote>(
        host: Arc<dyn InviteHost>,
        remote_factory: RemoteFactory<R>,
        runtime: &tokio::runtime::Handle,
    ) -> TaskHandle {
        let changed = Arc::new(Notify::new());
        let wake = Arc::clone(&changed);
        let unsubscribe = host.subscribe(Box::new(move |event: &Event| {
            if matches!(event, Event::InviteChanged) {
                wake.notify_one();
            }
        }));
        let subscription = Subscription::new(unsubscribe);
        let ends = Ends(subscription.clone());
        let task = runtime.spawn(async move {
            let _ends = ends;
            run_invite(host, remote_factory, changed).await;
        });
        TaskHandle::new(task, Some(subscription))
    }
}

/// Starts an [`InviteTask`] when the core asks for one.
pub struct InviteTasks;

impl InviteTasks {
    /// Listens for `InviteRequested` and runs the task; a new request replaces a task that still
    /// runs. Dropping the handle stops the listener and the task.
    #[must_use]
    pub fn spawn<H: InviteHost, R: SyncRemote>(
        host: Arc<H>,
        remote_factory: RemoteFactory<R>,
        runtime: tokio::runtime::Handle,
    ) -> TaskHandle {
        let (requests, mut received) = mpsc::unbounded_channel();
        let unsubscribe = InviteHost::subscribe(
            host.as_ref(),
            Box::new(move |event: &Event| {
                if matches!(event, Event::InviteRequested) {
                    let _ = requests.send(());
                }
            }),
        );
        let subscription = Subscription::new(unsubscribe);
        let ends = Ends(subscription.clone());
        let spawner = runtime.clone();
        let task = runtime.spawn(async move {
            let _ends = ends;
            let mut running: Option<TaskHandle> = None;
            while received.recv().await.is_some() {
                tracing::debug!("invite task starting");
                let host: Arc<dyn InviteHost> = Arc::clone(&host) as Arc<dyn InviteHost>;
                running = Some(InviteTask::spawn(
                    host,
                    Arc::clone(&remote_factory),
                    &spawner,
                ));
            }
            drop(running);
        });
        TaskHandle::new(task, Some(subscription))
    }
}

fn over(state: InviteSessionState) -> bool {
    matches!(
        state,
        InviteSessionState::Idle
            | InviteSessionState::Done
            | InviteSessionState::Failed
            | InviteSessionState::Cancelled
    )
}

/// The mailbox role byte of a side: the inviter opened the mailbox, the invitee types the code.
fn role_byte(side: InviteSide) -> u8 {
    match side {
        InviteSide::Inviter => PAIR_ROLE_OPENER,
        InviteSide::Invitee => PAIR_ROLE_TYPIST,
    }
}

fn peer(side: InviteSide) -> InviteSide {
    match side {
        InviteSide::Inviter => InviteSide::Invitee,
        InviteSide::Invitee => InviteSide::Inviter,
    }
}

/// What a 404 of an invite route is shown as; every other failure keeps its own words.
fn invite_failure(error: SyncError) -> Failure {
    match error {
        SyncError::NotFound => Failure::Message(INVALID_INVITE.to_owned()),
        SyncError::Idle => Failure::Message(IDLE_DEVICE.to_owned()),
        other => Failure::Remote(other),
    }
}

/// Runs a remote call that needs the session; a refusal that dropped the session authenticates once
/// and retries once (`$call` is evaluated again).
macro_rules! authed {
    ($session:expr, $call:expr) => {
        match retried(|| $call).await {
            Err(SyncError::Unauthorized) => {
                $session.authenticate().await?;
                retried(|| $call).await
            }
            other => other,
        }
    };
}

async fn run_invite<R: SyncRemote>(
    host: Arc<dyn InviteHost>,
    factory: RemoteFactory<R>,
    changed: Arc<Notify>,
) {
    let Ok(info) = host_call(&host, |host| host.invite_info()).await else {
        return;
    };
    if over(info.state) {
        return;
    }
    let remote = match info.server_url.as_deref().map(|url| factory(url)) {
        Some(Ok(remote)) => remote,
        Some(Err(error)) => {
            fail(&host, &error.to_string()).await;
            return;
        }
        None => {
            fail(&host, "no server for the invitation").await;
            return;
        }
    };
    let mut session = Invite {
        host: Arc::clone(&host),
        remote,
        side: info.side,
        flow: info.flow,
        mailbox: None,
        changed,
        authed: false,
        conflicts: 0,
    };
    let outcome = session.run().await;
    let close = match &outcome {
        // The invitee is through: the inviter closes the mailbox when its bundle is posted.
        Ok(InviteSessionState::Done) => false,
        Ok(_) => true,
        Err(failure) if failure.locked() => {
            tracing::debug!("the workspace is locked: invite task stopped");
            return;
        }
        Err(failure) => {
            tracing::info!(side = ?info.side, flow = ?info.flow, "invite failed");
            // The core may have ended the session itself (a message that did not open).
            let ended = host_call(&host, |host| host.invite_info())
                .await
                .is_ok_and(|info| over(info.state));
            if !ended {
                fail(&host, &failure.message()).await;
            }
            !matches!(failure, Failure::Message(text) if text == CODE_EXPIRED)
        }
    };
    if close
        && let Some(id) = session.mailbox
        && session.remote.pair_close(id).await.is_err()
    {
        tracing::debug!("the invite mailbox could not be closed");
    }
    tracing::debug!(side = ?info.side, flow = ?info.flow, "invite task ended");
}

async fn fail(host: &Arc<dyn InviteHost>, message: &str) {
    let message = message.to_owned();
    if host_call(host, move |host| host.invite_failed(&message))
        .await
        .is_err()
    {
        tracing::debug!("the invite failure could not be recorded");
    }
}

struct Invite<R> {
    host: Arc<dyn InviteHost>,
    remote: Arc<R>,
    side: InviteSide,
    flow: InviteFlow,
    mailbox: Option<[u8; 6]>,
    changed: Arc<Notify>,
    /// A session of this device is open on the remote.
    authed: bool,
    /// Lost compare-and-swaps in a row (the inviter's `Invite` entry).
    conflicts: u32,
}

impl<R: SyncRemote> Invite<R> {
    /// Runs the session to its end; answers the final state.
    async fn run(&mut self) -> Result<InviteSessionState, Failure> {
        match (self.flow, self.side) {
            (InviteFlow::Code, _) => self.run_code().await,
            (InviteFlow::Link, InviteSide::Invitee) => self.run_link().await,
            // A link has no session on the inviter's side: creating it is one request.
            (InviteFlow::Link, InviteSide::Inviter) => Ok(InviteSessionState::Idle),
        }
    }

    // ----- the code flow: the mailbox -------------------------------------------------------------

    async fn run_code(&mut self) -> Result<InviteSessionState, Failure> {
        let (id, mut next, mut announced) = self.open_mailbox().await?;
        let mine = role_byte(self.side);
        let theirs = peer(self.side);
        let mut joined = false;
        loop {
            // Armed before the state is read: a confirmation in between still wakes us.
            let changed = Arc::clone(&self.changed);
            let changed = changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();

            let info = host_call(&self.host, |host| host.invite_info()).await?;
            if over(info.state) {
                self.flush_abort(id, info.state).await;
                return Ok(info.state);
            }
            if self.side == InviteSide::Inviter {
                self.post_invite_entries().await?;
            }
            if self.side == InviteSide::Invitee
                && info.state == InviteSessionState::Joining
                && !joined
            {
                joined = true;
                self.finish_join(self.flow == InviteFlow::Code).await?;
                continue;
            }
            let outbox = host_call(&self.host, |host| host.invite_outbox()).await?;
            if !outbox.is_empty() {
                let bundle = self.side == InviteSide::Inviter
                    && info.state == InviteSessionState::Transferring;
                for (seq, bytes) in outbox {
                    let remote = &self.remote;
                    retried(|| remote.pair_post(id, mine, &bytes))
                        .await
                        .map_err(mailbox_failure)?;
                    host_call(&self.host, move |host| host.invite_posted(seq)).await?;
                }
                if bundle {
                    // The bundle is posted: what is left (the invitee's `Join`) the sync driver sees.
                    return Ok(InviteSessionState::Transferring);
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
                        host_call(&self.host, move |host| host.invite_mailbox_opened(id, expires_at))
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
                        host_call(&self.host, move |host| host.invite_receive(seq, theirs, &bytes))
                            .await?;
                    }
                    if fresh == 0 {
                        if page.closed {
                            return Err(Failure::Message(crate::devices::PEER_LEFT.to_owned()));
                        }
                        // A remote that answered at once with nothing: do not ask again straight
                        // away (the user can still wake us).
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

    /// The mailbox, the first message to read and whether the core knows its expiry.
    async fn open_mailbox(&mut self) -> Result<([u8; 6], u32, bool), Failure> {
        let known = host_call(&self.host, |host| host.invite_mailbox_id()).await?;
        match (self.side, known) {
            // An inviter whose mailbox is open already (the task was restarted).
            (InviteSide::Inviter, Some(id)) => {
                self.mailbox = Some(id);
                Ok((id, 1, true))
            }
            (InviteSide::Inviter, None) => {
                let outbox = host_call(&self.host, |host| host.invite_outbox()).await?;
                let Some((_, offer)) = outbox.into_iter().find(|(seq, _)| *seq == 0) else {
                    return Err(Failure::Core(ApiError::internal(
                        "the invite session has no offer to post",
                    )));
                };
                let remote = &self.remote;
                let (id, expires_at) = retried(|| remote.pair_create(&offer))
                    .await
                    .map_err(Failure::Remote)?;
                self.mailbox = Some(id);
                host_call(&self.host, move |host| {
                    host.invite_mailbox_opened(id, expires_at)
                })
                .await?;
                host_call(&self.host, |host| host.invite_posted(0)).await?;
                tracing::debug!("invite mailbox opened");
                Ok((id, 1, true))
            }
            (InviteSide::Invitee, Some(id)) => {
                self.mailbox = Some(id);
                Ok((id, 0, false))
            }
            (InviteSide::Invitee, None) => Err(Failure::Core(ApiError::internal(
                "the invite session has no mailbox",
            ))),
        }
    }

    /// The core ended the session (cancelled it, or failed it after the channel was open): the abort it
    /// queued tells the other side. Best effort.
    async fn flush_abort(&self, id: [u8; 6], state: InviteSessionState) {
        if !matches!(
            state,
            InviteSessionState::Cancelled | InviteSessionState::Failed
        ) {
            return;
        }
        let Ok(outbox) = host_call(&self.host, |host| host.invite_outbox()).await else {
            return;
        };
        let mine = role_byte(self.side);
        for (seq, bytes) in outbox {
            let remote = &self.remote;
            if retried(|| remote.pair_post(id, mine, &bytes))
                .await
                .is_err()
            {
                return;
            }
            if host_call(&self.host, move |host| host.invite_posted(seq))
                .await
                .is_err()
            {
                return;
            }
        }
    }

    // ----- the inviter ----------------------------------------------------------------------------

    /// The `Invite` entry (and whatever else the session minted) goes to the server by compare-and-swap
    /// before the bundle leaves. A lost swap is the sync driver's to repair (it drops what was minted on
    /// the old head and writes the entry again): the task waits and asks again.
    async fn post_invite_entries(&mut self) -> Result<(), Failure> {
        let Some(pending) = host_call(&self.host, |host| host.invite_membership_pending()).await?
        else {
            return Ok(());
        };
        self.publish_log().await?;
        let feed = pending.feed;
        let remote = Arc::clone(&self.remote);
        let posted = authed!(self, remote.membership_append(feed, &pending.entries));
        match posted {
            Ok(head) => {
                self.conflicts = 0;
                host_call(&self.host, move |host| host.invite_membership_posted(head)).await
            }
            Err(SyncError::Conflict(_)) => {
                self.conflicts += 1;
                if self.conflicts > MAX_CONFLICTS {
                    return Err(Failure::Message(INVITE_NOT_WRITTEN.to_owned()));
                }
                tracing::debug!("the invite's compare-and-swap was lost");
                Ok(())
            }
            Err(SyncError::NotFound | SyncError::Forbidden) => {
                Err(Failure::Message(CANNOT_INVITE.to_owned()))
            }
            Err(SyncError::Closed) => Err(Failure::Message("this space was closed".to_owned())),
            Err(error) => Err(Failure::Remote(error)),
        }
    }

    /// The inviter's unpublished log tail goes to the server before the `Invite` that names its
    /// devices (the server refuses an entry whose signer's log it lacks).
    async fn publish_log(&mut self) -> Result<(), Failure> {
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
                "create your recovery kit before inviting people".to_owned(),
            ));
        };
        let remote = &self.remote;
        let held = retried(|| remote.fetch_log(account, None))
            .await
            .map_err(Failure::Remote)?;
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
        host_call(&self.host, move |host| host.sync_log_published(up_to_seq)).await
    }

    // ----- the invitee ----------------------------------------------------------------------------

    /// The link flow: open and verify, wait for the preview's answer, join.
    async fn run_link(&mut self) -> Result<InviteSessionState, Failure> {
        let mut opened = false;
        loop {
            let changed = Arc::clone(&self.changed);
            let changed = changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();

            let info = host_call(&self.host, |host| host.invite_info()).await?;
            if over(info.state) {
                return Ok(info.state);
            }
            match info.state {
                InviteSessionState::Starting => {
                    if opened {
                        return Err(Failure::Message(NOT_VERIFIED.to_owned()));
                    }
                    opened = true;
                    let epoch = self.server_epoch().await?;
                    self.open_and_verify(epoch).await?;
                }
                InviteSessionState::Joining => {
                    self.finish_join(false).await?;
                }
                _ => {
                    // The preview waits for the user; anything else for the core.
                    let wait = if info.state == InviteSessionState::Previewing {
                        PREVIEW_TIMEOUT
                    } else {
                        Duration::from_secs(60)
                    };
                    if tokio::time::timeout(wait, changed.as_mut()).await.is_err()
                        && info.state == InviteSessionState::Previewing
                    {
                        return Err(Failure::Message(PREVIEW_TIMEOUT_MESSAGE.to_owned()));
                    }
                }
            }
        }
    }

    /// The server's epoch (what the signup and the open request sign).
    async fn server_epoch(&self) -> Result<[u8; 16], Failure> {
        let remote = &self.remote;
        let info = retried(|| remote.server_info())
            .await
            .map_err(Failure::Remote)?;
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(Failure::Message(WRONG_PROTOCOL.to_owned()));
        }
        Ok(info.server_epoch)
    }

    /// What the invitee does once it may join: the account, the session, the open and its
    /// verification (the code flow does them now; the link flow's came before the preview), the
    /// `Join`.
    async fn finish_join(&mut self, open: bool) -> Result<(), Failure> {
        let epoch = self.server_epoch().await?;
        self.signup(epoch).await?;
        self.authenticate().await?;
        let mut join = None;
        if open {
            join = self.open_and_verify(epoch).await?;
            let info = host_call(&self.host, |host| host.invite_info()).await?;
            if over(info.state) {
                return Ok(());
            }
        }
        let join = match join {
            Some(join) => join,
            None => host_call(&self.host, |host| host.invite_join())
                .await?
                .ok_or_else(|| {
                    Failure::Core(ApiError::internal("the invite has no join to post"))
                })?,
        };
        self.post_join(join).await
    }

    /// Creates the invitee's account with the share claim when the server does not have it, and tells
    /// the core the whole log is there.
    async fn signup(&self, epoch: [u8; 16]) -> Result<(), Failure> {
        let Some(signup) = host_call(&self.host, move |host| host.invite_signup(epoch)).await?
        else {
            return Ok(());
        };
        let remote = &self.remote;
        let created = retried(|| {
            remote.account_create_share(&signup.log, signup.invite_id, &signup.signature)
        })
        .await;
        match created {
            Ok(()) => {
                if let Some(last) = signup.log.len().checked_sub(1) {
                    let up_to_seq = last as u64;
                    host_call(&self.host, move |host| host.sync_log_published(up_to_seq)).await?;
                }
                tracing::debug!("account created from the invitation");
                Ok(())
            }
            // An earlier attempt created the account: the sync driver publishes what is missing.
            Err(SyncError::Conflict(_)) => Ok(()),
            Err(SyncError::Forbidden) => Err(Failure::Message(SIGNUP_REFUSED.to_owned())),
            Err(error) => Err(invite_failure(error)),
        }
    }

    /// Opens a session for this device, the core signing the challenge.
    async fn authenticate(&mut self) -> Result<(), Failure> {
        let Some((account, device)) = host_call(&self.host, |host| host.sync_identity()).await?
        else {
            return Err(Failure::Message(
                "create your recovery kit before joining a shared space".to_owned(),
            ));
        };
        let host = Arc::clone(&self.host);
        let signer: AuthSigner = Arc::new(move |message: &[u8]| host.sync_sign(message).ok());
        self.remote
            .authenticate(account, device, signer)
            .await
            .map_err(Failure::Remote)?;
        self.authed = true;
        tracing::debug!("session opened for the invitation");
        Ok(())
    }

    /// `invites/{id}/open`, then the core verifies the answer; while it needs a signer's device log
    /// the logs are fetched and it tries again. The code flow ends with the `Join` entry the core
    /// minted (`None` when the session ended meanwhile); the link flow ends in the preview.
    async fn open_and_verify(
        &mut self,
        epoch: [u8; 16],
    ) -> Result<Option<(FeedId, Vec<u8>)>, Failure> {
        let request = host_call(&self.host, move |host| host.invite_open_request(epoch))
            .await?
            .ok_or_else(|| Failure::Core(ApiError::internal("the invite has nothing to open")))?;
        let remote = &self.remote;
        let opened = retried(|| remote.invite_open(request.invite_id, &request.signature))
            .await
            .map_err(invite_failure)?;
        for _ in 0..MAX_LOG_ROUNDS {
            let answer = opened.clone();
            host_call(&self.host, move |host| host.invite_opened(&answer)).await?;
            let info = host_call(&self.host, |host| host.invite_info()).await?;
            if over(info.state) {
                return Ok(None);
            }
            match self.flow {
                InviteFlow::Link => {
                    if info.state != InviteSessionState::Starting {
                        return Ok(None);
                    }
                }
                InviteFlow::Code => {
                    if let Some(join) = host_call(&self.host, |host| host.invite_join()).await? {
                        return Ok(Some(join));
                    }
                }
            }
            // Verified or not yet: the core asks for the device logs it lacks.
            let needed = host_call(&self.host, |host| host.sync_needed_logs()).await?;
            if needed.is_empty() {
                break;
            }
            let remote = &self.remote;
            let chunks = retried(|| remote.logs_batch(&needed))
                .await
                .map_err(Failure::Remote)?;
            for chunk in chunks {
                let (account, entries) = (chunk.account, chunk.entries);
                host_call(&self.host, move |host| {
                    host.sync_foreign_log_received(account, &entries)
                })
                .await?;
            }
        }
        Err(Failure::Message(NOT_VERIFIED.to_owned()))
    }

    /// Posts the `Join` and tells the core the server took it. A lost compare-and-swap may be this
    /// very entry (a retry after a lost answer): the server's log says.
    async fn post_join(&mut self, (feed, bytes): (FeedId, Vec<u8>)) -> Result<(), Failure> {
        let remote = Arc::clone(&self.remote);
        let posted = authed!(
            self,
            remote.membership_append(feed, std::slice::from_ref(&bytes))
        );
        match posted {
            Ok(head) => host_call(&self.host, move |host| host.invite_joined(head)).await,
            Err(SyncError::Conflict(_)) => match self.join_already_posted(feed, &bytes).await {
                Some(head) => host_call(&self.host, move |host| host.invite_joined(head)).await,
                None => Err(Failure::Message(SPACE_CHANGED.to_owned())),
            },
            Err(SyncError::NotFound | SyncError::Forbidden) => {
                Err(Failure::Message(INVALID_INVITE.to_owned()))
            }
            Err(SyncError::Closed) => Err(Failure::Message("this space was closed".to_owned())),
            Err(error) => Err(Failure::Remote(error)),
        }
    }

    /// The membership head when the server's log holds exactly `entry` at its place.
    async fn join_already_posted(&self, feed: FeedId, entry: &[u8]) -> Option<MembershipHead> {
        let seq = peek_membership(entry).ok()?.seq;
        let after = seq.checked_sub(1);
        let remote = &self.remote;
        let page = retried(|| remote.membership_get(feed, after)).await.ok()?;
        page.entries
            .iter()
            .any(|item| item.bytes == entry)
            .then_some(page.head)
    }
}
