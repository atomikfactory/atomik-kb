//! The zero-knowledge sync server (ADR-0021, ADR-0045; `docs/development/milestone-06.md` and
//! `milestone-13.md`, "kb-server"): accounts from device logs, device-signature authentication,
//! the feeds of protocol v2 (a personal one per account and one per shared space) with their
//! membership log, access list, cursors and chain hash, snapshots, heads and chunks, on axum and
//! a plain SQLite file. It stores and relays ciphertext and signatures, enforces authentication,
//! roles, idempotency, quotas and pruning, and nothing else. Protocol v1 is retired: a database of
//! its days is refused and the server starts fresh.
//!
//! [`router`] is what the binary serves; [`Server`] gives tests a clock and a way to
//! run the pruning pass on demand. The HTTP surface is documented in
//! `deploy/kb-server/README.md` and next to the handlers in `src/api_v2.rs`.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use kb_types::Clock;

mod api;
mod api_v2;
mod codec;
mod error;
mod limits;
mod links;
mod pairing;
mod recovery;
mod store;
#[cfg(test)]
mod test_support;
mod viewer;
mod ws;

pub use limits::{TrustedProxy, TrustedProxyParseError};
pub use store::PruneReport;

/// How often the maintenance task prunes covered entries, expired sessions and expired
/// pairing mailboxes.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

const DAY_MS: i64 = 86_400_000;

/// Server configuration, read from the environment by the binary.
#[derive(Clone)]
pub struct Config {
    /// `KB_SERVER_LISTEN`, default `127.0.0.1:8787`.
    pub listen: SocketAddr,
    /// `KB_SERVER_DATA`: directory holding `server.db`.
    pub data_dir: PathBuf,
    /// `KB_SERVER_INVITE_TOKENS`: comma-separated; at least one is needed to create
    /// accounts.
    pub invite_tokens: Vec<String>,
    /// `KB_SERVER_ACCOUNT_QUOTA_BYTES`, default 10 GiB.
    pub account_quota_bytes: u64,
    /// `KB_SERVER_PRUNE_GRACE_DAYS`, default 7: the days a snapshot waits before the entries it
    /// covers are pruned (and, with the acknowledgement of every member, only then).
    pub prune_grace_days: u32,
    /// `KB_SERVER_IDLE_DEVICE_DAYS`: refuse authentication to a device whose last
    /// session is older than this many days. Unset (or empty): off.
    pub idle_device_days: Option<u32>,
    /// `KB_SERVER_TRUSTED_PROXIES`: comma-separated addresses or CIDR networks of
    /// reverse proxies. A request from one of them is rate-limited by the client
    /// address in `X-Forwarded-For` (else `X-Real-IP`); headers from any other peer are
    /// ignored. Unset: none (every limit counts the TCP peer).
    pub trusted_proxies: Vec<TrustedProxy>,
    /// `KB_SERVER_SHARE_SIGNUPS`: accounts that share invites may create on this server, counted
    /// over all time (0 switches share signups off; default 20). Invites for existing accounts
    /// still work.
    pub share_signups: u32,
    /// `KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT`: accounts the invites of one inviting account may
    /// create (default 5).
    pub share_signups_per_account: u32,
    /// `KB_SERVER_VIEWER_DIR`: the build of `apps/viewer` that `GET /view` serves (M13 slice 4,
    /// ADR-0051). Unset or empty: `/view` answers 404 and everything else is as before.
    pub viewer_dir: Option<PathBuf>,
}

impl Config {
    /// Reads the configuration from the environment; missing values take the defaults,
    /// malformed ones are errors (the message names the variable, never its value).
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen = match std::env::var("KB_SERVER_LISTEN") {
            Ok(text) => text
                .parse()
                .map_err(|_| ConfigError::Invalid("KB_SERVER_LISTEN"))?,
            Err(_) => SocketAddr::from(([127, 0, 0, 1], 8787)),
        };
        let data_dir = std::env::var("KB_SERVER_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data"));
        let invite_tokens = std::env::var("KB_SERVER_INVITE_TOKENS")
            .map(|text| {
                text.split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let account_quota_bytes = match std::env::var("KB_SERVER_ACCOUNT_QUOTA_BYTES") {
            Ok(text) => text
                .parse()
                .map_err(|_| ConfigError::Invalid("KB_SERVER_ACCOUNT_QUOTA_BYTES"))?,
            Err(_) => 10 * 1024 * 1024 * 1024,
        };
        let prune_grace_days = match std::env::var("KB_SERVER_PRUNE_GRACE_DAYS") {
            Ok(text) => text
                .parse()
                .map_err(|_| ConfigError::Invalid("KB_SERVER_PRUNE_GRACE_DAYS"))?,
            Err(_) => kb_types::sync::PRUNE_GRACE_DAYS,
        };
        let idle_device_days = match std::env::var("KB_SERVER_IDLE_DEVICE_DAYS") {
            Ok(text) if !text.trim().is_empty() => Some(
                text.trim()
                    .parse()
                    .ok()
                    .filter(|days: &u32| *days > 0)
                    .ok_or(ConfigError::Invalid("KB_SERVER_IDLE_DEVICE_DAYS"))?,
            ),
            _ => None,
        };
        let trusted_proxies = match std::env::var("KB_SERVER_TRUSTED_PROXIES") {
            Ok(text) => text
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::parse)
                .collect::<Result<Vec<TrustedProxy>, _>>()
                .map_err(|_| ConfigError::Invalid("KB_SERVER_TRUSTED_PROXIES"))?,
            Err(_) => Vec::new(),
        };
        let share_signups = match std::env::var("KB_SERVER_SHARE_SIGNUPS") {
            Ok(text) if !text.trim().is_empty() => text
                .trim()
                .parse()
                .map_err(|_| ConfigError::Invalid("KB_SERVER_SHARE_SIGNUPS"))?,
            _ => kb_types::sharing::SHARE_SIGNUPS_DEFAULT,
        };
        let share_signups_per_account = match std::env::var("KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT") {
            Ok(text) if !text.trim().is_empty() => text
                .trim()
                .parse()
                .map_err(|_| ConfigError::Invalid("KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT"))?,
            _ => kb_types::sharing::SHARE_SIGNUPS_PER_ACCOUNT_DEFAULT,
        };
        let viewer_dir = std::env::var("KB_SERVER_VIEWER_DIR")
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .map(PathBuf::from);
        Ok(Self {
            listen,
            data_dir,
            invite_tokens,
            account_quota_bytes,
            prune_grace_days,
            idle_device_days,
            trusted_proxies,
            share_signups,
            share_signups_per_account,
            viewer_dir,
        })
    }
}

/// A configuration problem; names the variable, never its value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not valid")]
    Invalid(&'static str),
}

impl core::fmt::Debug for Config {
    /// Invite tokens are secrets: only their number is shown.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("data_dir", &self.data_dir)
            .field("invite_tokens", &self.invite_tokens.len())
            .field("account_quota_bytes", &self.account_quota_bytes)
            .field("prune_grace_days", &self.prune_grace_days)
            .field("idle_device_days", &self.idle_device_days)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("share_signups", &self.share_signups)
            .field("share_signups_per_account", &self.share_signups_per_account)
            .field("viewer_dir", &self.viewer_dir)
            .finish()
    }
}

/// The wall clock.
struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

/// An opened server: its store, settings and clock. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Server {
    state: Arc<api::AppState>,
    grace_ms: i64,
}

impl Server {
    /// Opens (creating when needed) `server.db` in `config.data_dir`, with the wall
    /// clock.
    pub fn open(config: &Config) -> Result<Self, ServerError> {
        Self::open_with_clock(config, Arc::new(SystemClock))
    }

    /// Opens the store with an injected clock (tests: challenge and session expiry,
    /// pruning).
    pub fn open_with_clock(config: &Config, clock: Arc<dyn Clock>) -> Result<Self, ServerError> {
        let store = store::Store::open(&config.data_dir, clock.now_ms())?;
        let server_epoch = store.server_epoch();
        let state = api::AppState {
            store: Arc::new(Mutex::new(store)),
            server_epoch,
            settings: api::Settings {
                invite_tokens: config.invite_tokens.clone(),
                account_quota_bytes: config.account_quota_bytes,
                idle_device_days: config.idle_device_days,
                trusted_proxies: config.trusted_proxies.clone(),
                share_signups: config.share_signups,
                share_signups_per_account: config.share_signups_per_account,
                viewer_dir: config.viewer_dir.clone(),
            },
            clock,
            hub: ws::Hub::new(),
            mailbox_events: tokio::sync::watch::Sender::new(0),
            limiter: limits::RateLimiter::default(),
        };
        Ok(Self {
            state: Arc::new(state),
            grace_ms: i64::from(config.prune_grace_days).saturating_mul(DAY_MS),
        })
    }

    /// The random id of this server's storage (ADR-0021 §3).
    #[must_use]
    pub fn server_epoch(&self) -> [u8; 16] {
        self.state.server_epoch
    }

    /// The router over this server's state (`/v2` and the join page).
    pub fn router(&self) -> axum::Router {
        api::router(Arc::clone(&self.state))
    }

    /// How many `subscribe` frames the WebSockets have handled, accepted or not. The socket has no
    /// acknowledgement, so the tests wait on this counter to know a subscription took effect.
    #[doc(hidden)]
    #[must_use]
    pub fn realtime_subscribes_handled(&self) -> u64 {
        self.state.hub.subscribes_handled()
    }

    /// Publishes `count` notices no connection wants, so that connections that are slow to read lag
    /// and have to be told where their feeds stand. For the tests of that path.
    #[doc(hidden)]
    pub fn realtime_flood(&self, count: usize) {
        self.state.hub.flood(count);
    }

    /// One pruning pass now (blocking): deletes the payload of entries covered by a snapshot older
    /// than the grace period that every member acknowledged, closed feeds past their read window,
    /// and expired challenges, sessions and pairing mailboxes.
    pub fn prune_now(&self) -> Result<PruneReport, ServerError> {
        prune(&self.state, self.grace_ms)
    }

    /// The bytes an account stores (entries, snapshots, chunks, heads, its log), as the
    /// quota counts them; 0 for an unknown account.
    pub fn account_bytes_used(&self, account: kb_types::AccountId) -> Result<u64, ServerError> {
        let store = self
            .state
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        store
            .account_bytes(account)
            .map_err(|error| ServerError::Storage(error.message))
    }

    /// Starts the hourly maintenance task on the current tokio runtime (nothing happens
    /// outside one). The task ends once every router of this server is dropped.
    pub fn spawn_maintenance(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("no async runtime: pruning is not scheduled");
            return;
        };
        let weak: Weak<api::AppState> = Arc::downgrade(&self.state);
        let grace_ms = self.grace_ms;
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(PRUNE_INTERVAL);
            loop {
                interval.tick().await;
                let Some(state) = weak.upgrade() else {
                    return;
                };
                let outcome = tokio::task::spawn_blocking(move || prune(&state, grace_ms)).await;
                match outcome {
                    Ok(Ok(report)) => tracing::info!(
                        entries = report.entries,
                        bytes = report.bytes,
                        expired = report.expired,
                        mailboxes = report.mailboxes,
                        "maintenance pass"
                    ),
                    Ok(Err(error)) => tracing::error!(%error, "maintenance pass failed"),
                    Err(_) => tracing::error!("maintenance task failed"),
                }
            }
        });
    }
}

fn prune(state: &api::AppState, grace_ms: i64) -> Result<PruneReport, ServerError> {
    let now = state.now_ms();
    state.limiter.sweep(now);
    let mut store = state.store.lock().unwrap_or_else(PoisonError::into_inner);
    let mut report = store
        .prune(now)
        .map_err(|error| ServerError::Storage(error.message))?;
    // The feeds: a device is idle after the operator's limit, 30 days when unset.
    let idle_ms = state.settings.idle_limit_ms().unwrap_or_else(|| {
        i64::from(kb_types::sharing::PRUNE_IDLE_DEFAULT_DAYS).saturating_mul(DAY_MS)
    });
    let feeds = store
        .prune_feeds(now, grace_ms, idle_ms)
        .map_err(|error| ServerError::Storage(error.message))?;
    report.entries = report.entries.saturating_add(feeds.entries);
    report.bytes = report.bytes.saturating_add(feeds.bytes);
    report.feeds = feeds.feeds;
    Ok(report)
}

/// Builds the axum router for a configured server: opens the store (creating the server
/// epoch on first start) and schedules the hourly pruning pass when a tokio runtime is
/// running. The returned router is what the binary serves and what the tests drive
/// in-process.
pub fn router(config: &Config) -> Result<axum::Router, ServerError> {
    let server = Server::open(config)?;
    server.spawn_maintenance();
    Ok(server.router())
}

/// Server-side failures.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("cannot use the data directory: {0}")]
    Io(String),
    #[error("the operating system's random source failed")]
    Random,
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),
}
