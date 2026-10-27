//! The plumbing the v2 driver ([`crate::v2::SyncDriver`]) and the invite task share: the handle of a
//! running loop ([`DriverHandle`]), the schedule of cycles ([`Plan`]: debounce, pull timer, backoff
//! while offline), the remote factory and the messages a failed cycle reports.
//!
//! **Triggers.** `SyncRequested` (local edits, `SyncNow`) schedules a full cycle after
//! the host's debounce (`SyncHost::sync_push_debounce_ms`: [`PUSH_DEBOUNCE_MS`] of quiet, or
//! `PUSH_DEBOUNCE_PRESENCE_MS` while a page has a remote presence; at most [`MAX_DEBOUNCE`] after the
//! first request of a burst); `SyncChanged` makes the driver re-read `sync_info` (a new server
//! address recreates the remote, `configured == false` idles it); the realtime socket's
//! `Connected` and a timer every [`PULL_INTERVAL_MS`] schedule a pull (the socket's `changed`
//! frames and the entries it cannot apply pull the one feed they name, outside the cycle, see
//! [`crate::realtime`]). A pull also pushes when `pending_entries` says something waits. A cycle
//! whose chunk transfers used the whole transfer budget is followed at once by a pull (no pull
//! interval), and so on until a cycle finds less work than the budget.
//!
//! **Failures** end the cycle. `Unavailable` (and transport errors) set `Offline` and gate
//! the next cycle behind an exponential backoff of 1 s to 5 min, reset by a successful
//! cycle; any other remote error and any core error set `Error` with the message and the
//! next trigger retries; `Locked` sets nothing (the shell stops the driver).
//!
//! The cycle itself, feed by feed, is described in [`crate::v2`].

use std::sync::Arc;
use std::time::Duration;

use kb_types::sync::{PULL_INTERVAL_MS, PUSH_DEBOUNCE_MS};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::NetError;
use crate::realtime::RealtimeHandle;

/// Builds the remote for a server address (`HttpRemote::new` in the app).
pub type RemoteFactory<R> = Arc<dyn Fn(&str) -> Result<Arc<R>, NetError> + Send + Sync>;

/// The longest a burst of requests can delay a push.
pub const MAX_DEBOUNCE: Duration = Duration::from_millis(PUSH_DEBOUNCE_MS * 10);
/// First delay after the server could not be reached.
pub const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// Longest delay between attempts while offline.
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);

/// What the user is told when the device has no account identity yet.
pub const NO_KIT: &str = "create your recovery kit before syncing";
/// What the user is told when the server refused the invite token.
pub const INVITE_TOKEN_REFUSED: &str = "the server refused the invite token";
/// What the user is told when the server refused this device as idle (ADR-0022 §6).
pub const IDLE_DEVICE: &str = "this device was idle too long; pair it again";
/// What the user is told when the server refused an attachment chunk for the account's
/// storage quota (ADR-0023).
pub const QUOTA_FULL: &str = "the server's storage quota is full";

/// A running driver. Dropping it stops the loop (at its next await point), unsubscribes
/// from the host and closes the remote's realtime socket.
pub struct DriverHandle {
    stop: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
    unsubscribe: Option<Box<dyn FnOnce() + Send>>,
    realtime: RealtimeHandle,
}

impl DriverHandle {
    /// A handle for the loop of another driver (the v2 driver reuses this type); `realtime` is the
    /// slot the loop fills with its socket's commands.
    pub(crate) fn new(
        stop: watch::Sender<bool>,
        task: JoinHandle<()>,
        unsubscribe: Box<dyn FnOnce() + Send>,
        realtime: RealtimeHandle,
    ) -> Self {
        Self {
            stop: Some(stop),
            task: Some(task),
            unsubscribe: Some(unsubscribe),
            realtime,
        }
    }

    /// Stops the driver; never waits for it (the caller may hold the core's lock).
    pub fn stop(mut self) {
        self.shutdown();
    }

    /// Whether the loop is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    /// The handle onto this driver's realtime socket (M13 slice 3, [`crate::realtime`]): how the
    /// shell sends this device's presence. Clones share the socket; it works only while the driver
    /// has one open.
    #[must_use]
    pub fn realtime(&self) -> RealtimeHandle {
        self.realtime.clone()
    }

    fn shutdown(&mut self) {
        // At once: the task ends at its next await point, and presence sent meanwhile has no socket.
        self.realtime.detach_all();
        if let Some(unsubscribe) = self.unsubscribe.take() {
            unsubscribe();
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
            tracing::debug!("sync driver stopped");
        }
    }
}

impl Drop for DriverHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ----- scheduling ------------------------------------------------------------------------------

/// Exponential backoff between attempts while offline.
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    next: Duration,
}

impl Backoff {
    pub(crate) fn new() -> Self {
        Self { next: BACKOFF_MIN }
    }

    /// The delay before the next attempt; doubles up to [`BACKOFF_MAX`].
    pub(crate) fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        delay
    }

    pub(crate) fn reset(&mut self) {
        self.next = BACKOFF_MIN;
    }
}

/// What a cycle is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Everything: log, push, snapshots, pull, heads.
    Full,
    /// Pull and heads (and push anyway when entries are pending).
    Pull,
}

/// How a cycle ended, for the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It ran to the end (`Idle` or `Forked`).
    Synced,
    /// It ran to the end (`Idle`) and its chunk transfers used the whole budget: more are
    /// likely waiting, so a pull follows at once.
    Busy,
    /// The server could not be reached.
    Offline,
    /// Another failure (`Error`), or the core was locked.
    Failed,
    /// Sync is not configured.
    Idle,
}

/// When the next cycles are due.
pub(crate) struct Plan {
    /// Sync is configured: cycles run.
    pub(crate) active: bool,
    /// A full cycle, debounced.
    full_due: Option<Instant>,
    /// When the current burst of requests began.
    burst_start: Option<Instant>,
    /// A pull (periodic, or announced by a ping).
    pull_due: Option<Instant>,
    /// While offline, nothing runs before this.
    retry_at: Option<Instant>,
    backoff: Backoff,
}

impl Plan {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            full_due: None,
            burst_start: None,
            pull_due: None,
            retry_at: None,
            backoff: Backoff::new(),
        }
    }

    pub(crate) fn next_wake(&self) -> Option<Instant> {
        if !self.active {
            return None;
        }
        let due = match (self.full_due, self.pull_due) {
            (Some(full), Some(pull)) => full.min(pull),
            (full, pull) => full.or(pull)?,
        };
        Some(self.retry_at.map_or(due, |retry| due.max(retry)))
    }

    /// Sync became configured: a full cycle at once.
    pub(crate) fn activate(&mut self, now: Instant) {
        *self = Self::new();
        self.active = true;
        self.full_due = Some(now);
    }

    pub(crate) fn deactivate(&mut self) {
        *self = Self::new();
    }

    /// `SyncRequested`: a full cycle after `debounce` (the host's hint: [`PUSH_DEBOUNCE_MS`], or less
    /// while a page has a remote presence).
    pub(crate) fn request(&mut self, now: Instant, debounce: Duration) {
        if !self.active {
            return;
        }
        let start = *self.burst_start.get_or_insert(now);
        self.full_due = Some((now + debounce).min(start + MAX_DEBOUNCE));
    }

    /// A pull no later than `at` (the socket was connected, or ended; a pull that cannot wait is due
    /// now).
    pub(crate) fn pull_by(&mut self, at: Instant) {
        if self.active {
            self.pull_due = Some(self.pull_due.map_or(at, |due| due.min(at)));
        }
    }

    /// What runs now.
    pub(crate) fn take(&mut self, now: Instant) -> Kind {
        self.pull_due = None;
        if self.full_due.is_some_and(|due| due <= now) {
            self.full_due = None;
            self.burst_start = None;
            Kind::Full
        } else {
            Kind::Pull
        }
    }

    pub(crate) fn finished(&mut self, outcome: Outcome, now: Instant) {
        self.pull_due = Some(if outcome == Outcome::Busy {
            now
        } else {
            now + Duration::from_millis(PULL_INTERVAL_MS)
        });
        match outcome {
            Outcome::Offline => {
                let delay = self.backoff.next_delay();
                self.retry_at = Some(now + delay);
                // The retry is a full cycle: whatever was pending still is.
                self.full_due = Some(self.full_due.map_or(now, |due| due.min(now)));
            }
            Outcome::Synced | Outcome::Busy | Outcome::Failed | Outcome::Idle => {
                self.retry_at = None;
                self.backoff.reset();
            }
        }
    }
}

/// Whether the cycle goes on after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Continue,
    /// The step reported its outcome itself (through the core).
    Stop,
}

pub(crate) async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
