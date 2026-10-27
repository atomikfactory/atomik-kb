//! The realtime socket as the driver and the shell see it (M13 slice 3, ADR-0050,
//! `docs/development/milestone-13.md` sections 3.1 and 3.2): one WebSocket per device (`GET /v2/ws`),
//! subscribed to every feed of the account, delivering `changed` pings, inline entries and presence
//! frames, and sending this device's presence. The frames and their rules are `kb_sync::v2::ws`.
//!
//! - [`Realtime`] is what a [`SyncRemote`](crate::v2::SyncRemote) hands the driver when it opens the
//!   socket (`SyncRemote::realtime`): a receiver of [`RealtimeEvent`]s and a sender of
//!   [`RealtimeCommand`]s. The remote owns the connection: it connects, authenticates with the
//!   session's token, **sends the latest `Subscribe` after every (re)connection**, reconnects with
//!   backoff (1 s to 1 min) and ends (the receiver yields `None`) when the commands' sender is dropped.
//!   `HttpRemote` opens a real WebSocket (`crate::http`); the reference `MemoryRemote` bridges its
//!   `MemorySocket` ([`memory_socket`]), which cannot reconnect: when it closes, the receiver yields
//!   `None` and the driver opens another one. A socket the server refuses (a session that ended, a device
//!   that was revoked: HTTP 401 or 403 at the upgrade) is not retried with that token and says
//!   [`RealtimeEvent::Refused`]: the driver runs a cycle, which authenticates again or reports why it cannot.
//! - The driver subscribes to every feed of `sync_feeds()` (the personal feed too: the realtime path also
//!   serves a person's own devices), again when the list changes and once per cycle that posted a
//!   membership entry (the server checks a subscription against the access list when it receives it, and
//!   a feed the account just made is not on it before the `Create` is posted); routes an
//!   [`RealtimeEvent::Entry`] to `SyncHost::sync_feed_apply_inline` and an
//!   [`RealtimeEvent::Changed`] (or an answer `InlineOutcome::Pull`) to a pull of that one feed, outside
//!   the cycle; hands an [`RealtimeEvent::Presence`] to `SyncHost::presence_received`; runs a pull cycle
//!   after every [`RealtimeEvent::Connected`] (frames may have been missed while the socket was down);
//!   calls `SyncHost::presence_expire` about every five seconds while it has a socket (also while that
//!   socket reconnects, so that a peer's avatar times out when this device lost its connection); and
//!   asks `SyncHost::sync_push_debounce_ms` for the debounce of each `SyncRequested`. While the socket
//!   is down the pull timer of the cycle is all there is: correctness never depends on the socket.
//! - [`RealtimeHandle`] is the shell's way to send this device's presence: `DriverHandle::realtime()`
//!   gives a cloneable handle onto the driver's current socket; a frame sent while no socket is up is
//!   dropped (presence is best effort).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use kb_sync::v2::ws::{ENTRY_FRAME_PREFIX_LEN, PRESENCE_FRAME_PREFIX_LEN};
use kb_sync::v2::{InlineEntry, MemorySocket, PresenceFrame, ServerFrame};
use kb_types::FeedId;
use kb_types::sharing::{INLINE_ENTRY_MAX_BYTES, PRESENCE_MAX_FRAME_BYTES};
use tokio::sync::mpsc;

/// Frames of the socket the driver has not read yet. A cycle may run for a while while the socket is
/// read meanwhile; a full queue holds the socket back (the server survives a slow reader: it lags the
/// connection's receiver and says `changed` for every feed).
pub const EVENT_QUEUE: usize = 256;
/// Commands the socket has not handled yet. The senders use `try_send`: a full queue drops the frame,
/// presence is best effort.
pub const COMMAND_QUEUE: usize = 64;
/// How often the driver asks the core to drop presence that timed out.
pub const EXPIRE_INTERVAL: Duration = Duration::from_secs(5);
/// The largest message the WebSocket client reads: the binary frame of the largest inline entry
/// (`INLINE_ENTRY_MAX_BYTES` and the 65-byte frame prefix) with room to spare.
pub const MAX_WS_MESSAGE: usize = 2 * INLINE_ENTRY_MAX_BYTES;
/// How often the bridge of the reference remote looks for frames (its socket has no wake-up).
const MEMORY_POLL: Duration = Duration::from_millis(2);

const _: () = assert!(MAX_WS_MESSAGE > INLINE_ENTRY_MAX_BYTES + ENTRY_FRAME_PREFIX_LEN);

/// What the server told the driver over the socket, one decoded frame at a time, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeEvent {
    /// The socket is connected and its subscription was sent (also after a reconnection): whatever
    /// happened while it was down was missed, so the driver pulls.
    Connected,
    /// The text frame `changed`: the feed's newest cursor (an item the server did not relay inline, or
    /// a receiver that lagged). The driver pulls the feed when its cursor is lower.
    Changed {
        /// The feed.
        feed: FeedId,
        /// Its newest cursor.
        cursor: u64,
    },
    /// The binary frame `1`: a feed item with its position.
    Entry(InlineEntry),
    /// The binary frame `2`: another member's sealed presence.
    Presence(PresenceFrame),
    /// The socket dropped (the remote reconnects by itself); the driver stops expecting frames until
    /// the next [`RealtimeEvent::Connected`].
    Disconnected,
    /// The server refused the socket's session (HTTP 401 or 403 at the upgrade: the session expired or the
    /// device was revoked). That is an authentication failure, not a connection failure: the remote does
    /// not retry with that token, and the driver runs a cycle, which authenticates again or reports why
    /// it cannot.
    Refused,
}

/// What the driver tells the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeCommand {
    /// Replaces the subscription set (the remote remembers it and sends it again after a
    /// reconnection).
    Subscribe(Vec<FeedId>),
    /// Sends a presence frame (`kb_sync::v2::encode_presence_frame`). Best effort: dropped while the
    /// socket is down, and a frame the server's rate limit drops is gone.
    Presence(PresenceFrame),
}

/// The two ends of a realtime socket, as `SyncRemote::realtime` hands them to the driver.
#[derive(Debug)]
pub struct Realtime {
    /// The server's frames, in order; `None` when the remote gave the socket up for good.
    pub events: mpsc::Receiver<RealtimeEvent>,
    /// The driver's commands; dropping it ends the socket.
    pub commands: mpsc::Sender<RealtimeCommand>,
}

/// The shell's handle onto the driver's realtime socket (`DriverHandle::realtime`): cheap to clone,
/// usable from any thread, never blocks. The driver fills the shared slot while it has a socket and
/// empties it when the socket is gone, so a handle outlives the sockets and the drivers it was made for.
#[derive(Debug, Clone, Default)]
pub struct RealtimeHandle {
    slot: Arc<Mutex<Option<mpsc::Sender<RealtimeCommand>>>>,
}

impl RealtimeHandle {
    /// A handle that sends through `commands`, for hosts that test their side of presence without a driver
    /// (the receiver of `commands` is the socket: what it receives is what a driver's socket would).
    #[must_use]
    pub fn connected_to(commands: mpsc::Sender<RealtimeCommand>) -> Self {
        let handle = Self::default();
        handle.attach(commands);
        handle
    }

    /// Sends a sealed presence envelope (`kb_core::Core::presence_seal`) for `feed` through the socket.
    /// `true` when it was handed to the socket (not that the server relayed it), `false` when no
    /// socket is up or its queue is full: the frame is dropped, nothing waits for it.
    #[must_use]
    pub fn send_presence(&self, feed: FeedId, envelope: Vec<u8>) -> bool {
        // The frame is `2 | feed | envelope`; the server drops one over the limit.
        if PRESENCE_FRAME_PREFIX_LEN.saturating_add(envelope.len()) > PRESENCE_MAX_FRAME_BYTES {
            return false;
        }
        let Some(commands) = self.commands() else {
            return false;
        };
        commands
            .try_send(RealtimeCommand::Presence(PresenceFrame { feed, envelope }))
            .is_ok()
    }

    /// Whether a socket is attached now (it may still be connecting or reconnecting).
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.commands().is_some()
    }

    fn commands(&self) -> Option<mpsc::Sender<RealtimeCommand>> {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The driver has a socket: frames go to `commands`.
    pub(crate) fn attach(&self, commands: mpsc::Sender<RealtimeCommand>) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(commands);
    }

    /// The socket `commands` belongs to is gone (a newer socket stays attached).
    pub(crate) fn detach(&self, commands: &mpsc::Sender<RealtimeCommand>) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|attached| attached.same_channel(commands))
        {
            *slot = None;
        }
    }

    /// The driver is gone: nothing is attached any more.
    pub(crate) fn detach_all(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// The realtime socket of the reference remote: a task that bridges a [`MemorySocket`] (synchronous, no
/// wake-up, so polled every couple of milliseconds) onto the channels of [`Realtime`]. It starts with
/// [`RealtimeEvent::Connected`], carries `Subscribe` and `Presence` commands to the socket, ends with
/// [`RealtimeEvent::Disconnected`] when the socket closes (the reference server has no reconnection of
/// its own: the driver opens another socket) and closes the socket when the commands' sender is dropped.
/// `None` without a tokio runtime.
#[must_use]
pub(crate) fn memory_socket(socket: MemorySocket) -> Option<Realtime> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let (event_sender, events) = mpsc::channel(EVENT_QUEUE);
    let (commands, command_receiver) = mpsc::channel(COMMAND_QUEUE);
    runtime.spawn(bridge(socket, event_sender, command_receiver));
    Some(Realtime { events, commands })
}

async fn bridge(
    socket: MemorySocket,
    events: mpsc::Sender<RealtimeEvent>,
    mut commands: mpsc::Receiver<RealtimeCommand>,
) {
    if events.send(RealtimeEvent::Connected).await.is_err() {
        socket.close();
        return;
    }
    loop {
        // Everything delivered so far, in order, before anything else.
        while let Some(frame) = socket.recv() {
            let event = match frame {
                ServerFrame::Changed { feed, cursor } => RealtimeEvent::Changed { feed, cursor },
                ServerFrame::Entry(entry) => RealtimeEvent::Entry(entry),
                ServerFrame::Presence(frame) => RealtimeEvent::Presence(frame),
            };
            if events.send(event).await.is_err() {
                socket.close();
                return;
            }
        }
        if !socket.is_open() {
            let _ = events.send(RealtimeEvent::Disconnected).await;
            return;
        }
        tokio::select! {
            command = commands.recv() => match command {
                None => {
                    socket.close();
                    return;
                }
                Some(RealtimeCommand::Subscribe(feeds)) => {
                    if socket.subscribe(&feeds).is_err() {
                        // The session ended: the socket is of no use any more.
                        socket.close();
                    }
                }
                // The server's answer says why a frame was refused; a client says nothing.
                Some(RealtimeCommand::Presence(frame)) => {
                    let _ = socket.send_presence(&frame);
                }
            },
            () = tokio::time::sleep(MEMORY_POLL) => {}
        }
    }
}
