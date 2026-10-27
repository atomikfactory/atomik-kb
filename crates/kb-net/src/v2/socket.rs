//! The driver's side of the realtime socket (M13 slice 3, ADR-0050; the types and the rules are in
//! [`crate::realtime`]): the subscription, the frames the socket delivers (inline entries, `changed`
//! pings, presence, connection changes), the expiry tick of the presence table and the push debounce
//! the host asks for.
//!
//! Nothing here is needed for correctness: a socket that is down, a frame that is lost and an entry the
//! core cannot apply from here all fall back to a pull (of one feed, or the whole cycle), and the pull
//! timer runs whatever the socket does. Nothing is logged but counts and 8-character feed ids: never a
//! frame, an entry or a presence envelope.

use std::sync::Arc;
use std::time::Duration;

use kb_sync::v2::{FeedWork, InlineEntry, InlineOutcome, PresenceFrame};
use kb_types::FeedId;
use kb_types::sync::PUSH_DEBOUNCE_MS;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::SyncRemote;
use super::driver::{Driver, Link};
use crate::driver::Plan;
use crate::realtime::{EXPIRE_INTERVAL, Realtime, RealtimeCommand, RealtimeEvent, RealtimeHandle};

/// The driver's end of a realtime socket.
pub(super) struct Socket {
    events: mpsc::Receiver<RealtimeEvent>,
    commands: mpsc::Sender<RealtimeCommand>,
    handle: RealtimeHandle,
    /// The feeds the last `Subscribe` named (sorted), `None` before the first one and after one that
    /// could not be queued.
    subscribed: Option<Vec<FeedId>>,
    /// Between `Connected` and `Disconnected`.
    up: bool,
    /// When the presence table is asked to expire next.
    expire_at: Instant,
}

impl Socket {
    /// Takes a freshly opened socket and makes `handle` send through it.
    pub(super) fn new(realtime: Realtime, handle: RealtimeHandle) -> Self {
        handle.attach(realtime.commands.clone());
        Self {
            events: realtime.events,
            commands: realtime.commands,
            handle,
            subscribed: None,
            up: false,
            expire_at: Instant::now() + EXPIRE_INTERVAL,
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // The commands' sender goes with the socket: the remote's task ends, and a handle that
        // still points at it must not keep it alive.
        self.handle.detach(&self.commands);
    }
}

/// The next frame of the link's socket; pending while there is none, `None` when the remote gave it
/// up.
pub(super) async fn next_event<R>(link: &mut Option<Link<R>>) -> Option<RealtimeEvent> {
    match link.as_mut().and_then(|link| link.realtime.as_mut()) {
        Some(socket) => socket.events.recv().await,
        None => std::future::pending().await,
    }
}

impl<R: SyncRemote> Driver<R> {
    /// Opens the realtime socket when the link has none yet and the remote has one (it needs the
    /// session: call after authenticating).
    pub(super) fn open_socket(&mut self, remote: &Arc<R>) {
        let handle = self.realtime.clone();
        if let Some(link) = self.link.as_mut()
            && link.realtime.is_none()
            && let Some(realtime) = remote.realtime()
        {
            tracing::debug!("realtime socket opened");
            link.realtime = Some(Socket::new(realtime, handle));
        }
    }

    /// Tells the socket which feeds to subscribe to: every feed of `works`, the personal one included.
    /// Sent when the set differs from the last one, or always with `force` (the server checks the set
    /// against the access list when it receives it: a feed this account has just made, or just been
    /// admitted to, is not on it before).
    pub(super) fn subscribe_feeds(&mut self, works: &[FeedWork], force: bool) {
        let Some(socket) = self.link.as_mut().and_then(|link| link.realtime.as_mut()) else {
            return;
        };
        let mut feeds: Vec<FeedId> = works.iter().map(|work| work.feed).collect();
        feeds.sort_unstable();
        feeds.dedup();
        if !force && socket.subscribed.as_ref() == Some(&feeds) {
            return;
        }
        if socket.subscribed.is_none() && feeds.is_empty() {
            // The server starts with nothing subscribed.
            return;
        }
        let count = feeds.len();
        match socket
            .commands
            .try_send(RealtimeCommand::Subscribe(feeds.clone()))
        {
            Ok(()) => {
                tracing::debug!(feeds = count, "realtime subscription sent");
                socket.subscribed = Some(feeds);
            }
            // The next cycle sends it again.
            Err(_) => socket.subscribed = None,
        }
    }

    /// Whether the last `Subscribe` named `feed` (a server only relays what was subscribed to).
    fn subscribed_to(&self, feed: FeedId) -> bool {
        self.link
            .as_ref()
            .and_then(|link| link.realtime.as_ref())
            .and_then(|socket| socket.subscribed.as_ref())
            .is_some_and(|feeds| feeds.binary_search(&feed).is_ok())
    }

    /// When the presence table is asked to expire next, while the link has a socket.
    pub(super) fn expire_at(&self) -> Option<Instant> {
        self.link
            .as_ref()
            .and_then(|link| link.realtime.as_ref())
            .map(|socket| socket.expire_at)
    }

    /// Asks the core to drop the presence that timed out (it was not refreshed for
    /// `PRESENCE_TIMEOUT_SECONDS`), and schedules the next ask.
    pub(super) async fn expire(&mut self) {
        if let Some(socket) = self.link.as_mut().and_then(|link| link.realtime.as_mut()) {
            socket.expire_at = Instant::now() + EXPIRE_INTERVAL;
        }
        if self.host_call(|host| host.presence_expire()).await.is_err() {
            tracing::debug!("presence could not be expired");
        }
    }

    /// The push debounce now: the host's hint while there is a socket (presence can only arrive through
    /// one; the hint is 150 ms while a page has a remote presence and 500 ms otherwise), never more than
    /// [`PUSH_DEBOUNCE_MS`]. Read outside the core's event callback, which must not block.
    pub(super) async fn debounce(&self) -> Duration {
        let plain = Duration::from_millis(PUSH_DEBOUNCE_MS);
        if !self
            .link
            .as_ref()
            .is_some_and(|link| link.realtime.is_some())
        {
            return plain;
        }
        match self.host_call(|host| host.sync_push_debounce_ms()).await {
            Ok(ms) => Duration::from_millis(ms.min(PUSH_DEBOUNCE_MS)),
            Err(_) => plain,
        }
    }

    /// One frame of the socket.
    pub(super) async fn on_realtime(&mut self, event: RealtimeEvent, plan: &mut Plan) {
        match event {
            RealtimeEvent::Connected => {
                if let Some(link) = self.link.as_mut() {
                    link.socket_backoff.reset();
                    if let Some(socket) = link.realtime.as_mut() {
                        socket.up = true;
                    }
                }
                tracing::debug!("realtime socket connected");
                // Whatever happened while it was down, or before the subscription reached the
                // server, was not relayed: a pull finds it.
                plan.pull_by(Instant::now());
            }
            RealtimeEvent::Refused => {
                // The session is gone or the device refused: a cycle authenticates again (the socket
                // does not retry with the token the server refused) or reports why it cannot.
                tracing::debug!("the realtime socket was refused: a cycle follows");
                plan.pull_by(Instant::now());
            }
            RealtimeEvent::Disconnected => {
                if let Some(socket) = self.link.as_mut().and_then(|link| link.realtime.as_mut()) {
                    socket.up = false;
                }
                tracing::debug!("realtime socket disconnected");
            }
            RealtimeEvent::Changed { feed, cursor } => {
                // Only for a feed this socket subscribed to, and only for news: a frame that repeats
                // what was acted on, or announces less than this device reached, costs nothing.
                let news = self.subscribed_to(feed)
                    && self
                        .link
                        .as_mut()
                        .is_some_and(|link| link.announce(feed, cursor));
                if news {
                    tracing::debug!(feed = %feed.short_hex(), cursor, "the server announced a feed's growth");
                    self.pull_feed(plan, feed).await;
                }
            }
            RealtimeEvent::Entry(inline) => {
                if self.subscribed_to(inline.feed) {
                    self.inline(plan, inline).await;
                }
            }
            RealtimeEvent::Presence(PresenceFrame { feed, envelope }) => {
                if self
                    .host_call(move |host| host.presence_received(feed, &envelope))
                    .await
                    .is_err()
                {
                    tracing::debug!("a presence frame could not be handed over");
                }
            }
        }
    }

    /// The socket's receiver closed: the remote gave it up (the reference remote's socket does not
    /// reconnect). The next cycle opens another, after a pause that grows while sockets keep ending.
    pub(super) fn socket_ended(&mut self, plan: &mut Plan) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        if link.realtime.take().is_none() {
            return;
        }
        let delay = link.socket_backoff.next_delay();
        tracing::debug!(
            seconds = delay.as_secs(),
            "realtime socket ended: a pull reopens it"
        );
        plan.pull_by(Instant::now() + delay);
    }

    /// An entry the socket relayed: the core applies it when it is the next one of its feed, and
    /// otherwise says so and the feed is pulled.
    async fn inline(&mut self, plan: &mut Plan, inline: InlineEntry) {
        let feed = inline.feed;
        let outcome = self
            .host_call(move |host| host.sync_feed_apply_inline(feed, &inline))
            .await;
        match outcome {
            Ok(InlineOutcome::Applied(applied)) => {
                tracing::debug!(
                    feed = %feed.short_hex(),
                    applied = applied.applied,
                    skipped_own = applied.skipped_own,
                    pending = applied.pending,
                    rejected = applied.rejected,
                    cursor = applied.new_cursor,
                    "inline entry applied"
                );
                if let Some(link) = self.link.as_mut() {
                    link.note_cursor(feed, applied.new_cursor);
                }
                // Parked for a snapshot, or beyond a gap: the pull knows what to fetch.
                if applied.pending > 0 || applied.gaps > 0 {
                    self.pull_feed(plan, feed).await;
                }
            }
            Ok(InlineOutcome::Known) => {}
            Ok(InlineOutcome::Pull) => {
                tracing::debug!(feed = %feed.short_hex(), "an inline entry cannot be applied from here: pulling");
                self.pull_feed(plan, feed).await;
            }
            Err(_) => {
                tracing::debug!(feed = %feed.short_hex(), "an inline entry could not be offered to the core");
                plan.pull_by(Instant::now());
            }
        }
    }
}
