//! The realtime socket of the reference remote (M13 slice 3, ADR-0050, `docs/development/milestone-13.md`
//! section 3.1): what `GET /v2/ws` does, as a synchronous in-memory object for the fleet simulator of
//! `kb-core` and the tests of `kb-net`, with the same rules as the real server (the frames and their
//! semantics are in [`crate::v2::ws`]):
//!
//! - [`MemoryRemote::ws_open`] opens a socket for this connection's session (the account and device
//!   that authenticated); every append to a feed the socket subscribed to is delivered to it as an
//!   entry frame (`prev_cursor`, `cursor`, `chain_after`; an item over `INLINE_ENTRY_MAX_BYTES` as a
//!   `changed` frame), the appending device's own socket included. `prev_cursor` is the cursor of the
//!   item before it in the feed, a pruned one included;
//! - [`MemorySocket::subscribe`] replaces the subscription set and checks it against the access list
//!   (a feed the account may not read is ignored, silently, like the server). A removed or departed
//!   member's subscription ends at its `Remove` or `Leave`: the socket gets that entry and nothing
//!   after it;
//! - [`MemorySocket::send_presence`] relays a presence frame to the other subscribed sockets of the
//!   feed and stores nothing. It enforces the server's checks: the header's account is the session's,
//!   its feed is the frame's, 4 KiB per frame, 10 frames per second per socket on the remote's clock,
//!   and the sending socket is subscribed to the feed (a subscription is a checked membership, so
//!   only current members send, and never on the personal feed);
//! - frames are queued per socket in delivery order; [`MemorySocket::recv`] pops the oldest. The
//!   knobs [`MemoryRemote::ws_drop_next`] and [`MemoryRemote::ws_swap_next_two`] lose and reorder
//!   frames on demand;
//! - a socket lives as long as its session: authenticating again on the connection (which replaces
//!   the session), a device revoked in its account's log, `set_offline(true)` and `reset_data` close
//!   it, and so do [`MemorySocket::close`] and dropping it. A closed socket receives nothing.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use kb_types::FeedId;
use kb_types::sharing::{INLINE_ENTRY_MAX_BYTES, MAX_FEEDS_PER_ACCOUNT, PRESENCE_MAX_FRAME_BYTES};

use super::{Inner, MemoryRemote, Session};
use crate::SyncError;
use crate::v2::guard::Access;
use crate::v2::ws::{
    FrameLimiter, InlineEntry, PRESENCE_FRAME_PREFIX_LEN, PresenceFrame, ServerFrame, peek_presence,
};

/// One realtime socket of a [`MemoryRemote`] (the reference of the server's WebSocket).
pub struct MemorySocket {
    inner: Arc<Mutex<Inner>>,
    id: u64,
}

impl core::fmt::Debug for MemorySocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MemorySocket")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What the remote keeps of an open socket.
pub(super) struct SocketSlot {
    /// The account and device the socket was opened for.
    session: Session,
    /// The session token the socket depends on: the socket ends with it.
    token: [u8; 32],
    subscribed: BTreeSet<FeedId>,
    queue: VecDeque<ServerFrame>,
    /// The presence frames this socket sent, for the rate limit.
    limiter: FrameLimiter,
}

/// The state of the reordering knob (only the `testing` knobs arm it).
#[derive(Default)]
#[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
pub(super) enum WsSwap {
    /// Nothing to reorder.
    #[default]
    Off,
    /// The next frame to be delivered to a socket is held back.
    Armed,
    /// A frame is held for `socket`; the next one it gets is queued before it.
    Holding {
        /// The socket the held frame is for.
        socket: u64,
        /// The frame.
        frame: ServerFrame,
    },
}

impl Inner {
    /// Whether the session a socket was opened under is still in force and its device still active.
    fn socket_session_alive(&self, slot: &SocketSlot) -> bool {
        self.sessions.get(&slot.token).is_some_and(|stored| {
            stored.account == slot.session.account && stored.device == slot.session.device
        }) && self
            .accounts
            .get(&slot.session.account)
            .is_some_and(|account| account.state.active(slot.session.device).is_some())
    }

    /// Closes the sockets whose session ended (or all of them while the remote is offline); frames
    /// they had not received are dropped.
    pub(super) fn reap_sockets(&mut self) {
        let offline = self.offline;
        let dead: Vec<u64> = self
            .sockets
            .iter()
            .filter(|(_, slot)| offline || !self.socket_session_alive(slot))
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            self.sockets.remove(&id);
        }
        if let WsSwap::Holding { socket, .. } = &self.knobs.ws_swap
            && !self.sockets.contains_key(socket)
        {
            self.knobs.ws_swap = WsSwap::Off;
        }
    }

    /// Queues `frame` for socket `id`, as the testing knobs let it through: lost, held back or
    /// reordered with the next frame the socket gets.
    fn queue_frame(&mut self, id: u64, frame: ServerFrame) {
        if self.knobs.ws_drop > 0 {
            self.knobs.ws_drop -= 1;
            return;
        }
        let frame = match std::mem::take(&mut self.knobs.ws_swap) {
            WsSwap::Armed => {
                self.knobs.ws_swap = WsSwap::Holding { socket: id, frame };
                return;
            }
            WsSwap::Holding {
                socket,
                frame: held,
            } if socket == id => {
                if let Some(slot) = self.sockets.get_mut(&id) {
                    slot.queue.push_back(frame);
                    slot.queue.push_back(held);
                }
                return;
            }
            other => {
                self.knobs.ws_swap = other;
                frame
            }
        };
        if let Some(slot) = self.sockets.get_mut(&id) {
            slot.queue.push_back(frame);
        }
    }

    /// Delivers a feed item that was just stored to the sockets subscribed to its feed: the entry frame
    /// (the `changed` frame for an item over `INLINE_ENTRY_MAX_BYTES`) to the sockets of current
    /// members, and to a socket whose account the item removes (it reads the feed up to its `Remove`),
    /// whose subscription then ends. `prev_cursor` is the cursor of the item before it in the feed.
    pub(super) fn push_item(
        &mut self,
        feed: FeedId,
        prev_cursor: u64,
        cursor: u64,
        chain_after: [u8; 32],
        bytes: &[u8],
    ) {
        self.reap_sockets();
        let frame = if bytes.len() <= INLINE_ENTRY_MAX_BYTES {
            ServerFrame::Entry(InlineEntry {
                feed,
                prev_cursor,
                cursor,
                chain_after,
                entry: bytes.to_vec(),
            })
        } else {
            ServerFrame::Changed { feed, cursor }
        };
        let targets: Vec<(u64, Session)> = self
            .sockets
            .iter()
            .filter(|(_, slot)| slot.subscribed.contains(&feed))
            .map(|(id, slot)| (*id, slot.session))
            .collect();
        for (id, session) in targets {
            let deliver = match self.access(&session, feed) {
                Access::Member(_) => true,
                Access::Removed { cursor: end } => {
                    self.unsubscribe(id, feed);
                    cursor <= end
                }
                Access::Stranger => {
                    self.unsubscribe(id, feed);
                    false
                }
            };
            if deliver {
                self.queue_frame(id, frame.clone());
            }
        }
    }

    fn unsubscribe(&mut self, id: u64, feed: FeedId) {
        if let Some(slot) = self.sockets.get_mut(&id) {
            slot.subscribed.remove(&feed);
        }
    }
}

impl MemorySocket {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the socket's subscription set with the feeds this session's account may read (the
    /// others are ignored). `Unauthorized` when the session ended or the socket is closed;
    /// `Protocol` for more than `MAX_FEEDS_PER_ACCOUNT` feeds.
    pub fn subscribe(&self, feeds: &[FeedId]) -> Result<(), SyncError> {
        if feeds.len() > MAX_FEEDS_PER_ACCOUNT {
            return Err(SyncError::Protocol(format!(
                "a subscription names at most {MAX_FEEDS_PER_ACCOUNT} feeds"
            )));
        }
        let mut guard = self.lock();
        let inner = &mut *guard;
        inner.reap_sockets();
        let session = inner
            .sockets
            .get(&self.id)
            .ok_or(SyncError::Unauthorized)?
            .session;
        let allowed: BTreeSet<FeedId> = feeds
            .iter()
            .copied()
            .filter(|feed| {
                inner.feeds.contains_key(feed)
                    && matches!(inner.access(&session, *feed), Access::Member(_))
            })
            .collect();
        if let Some(slot) = inner.sockets.get_mut(&self.id) {
            slot.subscribed = allowed;
        }
        Ok(())
    }

    /// Sends a presence frame as the server would receive it: refused with `Authentication` when the
    /// header's account is not the session's and `Protocol` for a feed that differs from the header's
    /// or a frame over `PRESENCE_MAX_FRAME_BYTES`; `NotFound` for a feed this socket is not subscribed
    /// to (not a member's, or never subscribed, or the personal feed); the excess over the per-second
    /// limit is dropped and answers `Ok` (like the real server, which says nothing: it ignores a
    /// refused frame, keeps the connection open and logs a count; the reasons are answered here only so
    /// tests can assert them); otherwise relayed to the other subscribed sockets of the feed's
    /// current members (never this one, always the other devices of this account). `Unauthorized`
    /// when the session ended. Every call counts for the rate limit, refused or not.
    pub fn send_presence(&self, frame: &PresenceFrame) -> Result<(), SyncError> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        inner.reap_sockets();
        let now = inner.now_ms;
        let slot = inner
            .sockets
            .get_mut(&self.id)
            .ok_or(SyncError::Unauthorized)?;
        if !slot.limiter.allow(now) {
            return Ok(());
        }
        let session = slot.session;
        let subscribed = slot.subscribed.contains(&frame.feed);
        if PRESENCE_FRAME_PREFIX_LEN.saturating_add(frame.envelope.len()) > PRESENCE_MAX_FRAME_BYTES
        {
            return Err(SyncError::Protocol(format!(
                "a presence frame is at most {PRESENCE_MAX_FRAME_BYTES} bytes"
            )));
        }
        let header = peek_presence(&frame.envelope)?;
        if header.feed != frame.feed {
            return Err(SyncError::Protocol(
                "the presence header names another feed than the frame".to_owned(),
            ));
        }
        if header.account_id != session.account {
            return Err(SyncError::Authentication);
        }
        if !subscribed
            || frame.feed == FeedId::from_account(session.account)
            || !matches!(inner.access(&session, frame.feed), Access::Member(_))
        {
            return Err(SyncError::NotFound);
        }
        let targets: Vec<(u64, Session)> = inner
            .sockets
            .iter()
            .filter(|(id, other)| **id != self.id && other.subscribed.contains(&frame.feed))
            .map(|(id, other)| (*id, other.session))
            .collect();
        for (id, other) in targets {
            if matches!(inner.access(&other, frame.feed), Access::Member(_)) {
                inner.queue_frame(id, ServerFrame::Presence(frame.clone()));
            }
        }
        Ok(())
    }

    /// Pops the oldest frame delivered to this socket, if any.
    #[must_use]
    pub fn recv(&self) -> Option<ServerFrame> {
        let mut guard = self.lock();
        guard.reap_sockets();
        guard.sockets.get_mut(&self.id)?.queue.pop_front()
    }

    /// Whether the socket still receives (`false` after [`MemorySocket::close`], or when the
    /// connection's session was replaced by another `authenticate`).
    #[must_use]
    pub fn is_open(&self) -> bool {
        let mut guard = self.lock();
        guard.reap_sockets();
        guard.sockets.contains_key(&self.id)
    }

    /// Closes the socket; frames not yet received are dropped.
    pub fn close(&self) {
        self.lock().sockets.remove(&self.id);
    }
}

impl Drop for MemorySocket {
    fn drop(&mut self) {
        self.close();
    }
}

impl MemoryRemote {
    /// Opens a realtime socket for this connection's session, with an empty subscription set.
    /// `Unauthorized` without a session. Several sockets of one connection are allowed, but each
    /// ends when the connection's session is replaced by another `authenticate`: to have two devices
    /// receive at once give each its own connection (`clone`) and authenticate it once.
    pub fn ws_open(&self) -> Result<MemorySocket, SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        let token = self.session_token().ok_or(SyncError::Unauthorized)?;
        let inner = &mut *guard;
        inner.reap_sockets();
        let id = inner.next_socket;
        inner.next_socket = inner.next_socket.saturating_add(1);
        inner.sockets.insert(
            id,
            SocketSlot {
                session,
                token,
                subscribed: BTreeSet::new(),
                queue: VecDeque::new(),
                limiter: FrameLimiter::presence(),
            },
        );
        Ok(MemorySocket {
            inner: Arc::clone(&self.inner),
            id,
        })
    }

    /// The next `count` frames the server would deliver to any socket are lost (the simulator's
    /// dropped frames: the receivers must pull). Replaces the count still pending. Behind the
    /// `testing` feature.
    #[cfg(any(test, feature = "testing"))]
    pub fn ws_drop_next(&self, count: usize) {
        self.lock().knobs.ws_drop = count;
    }

    /// The next two frames the server would deliver to a socket arrive in the opposite order (the
    /// simulator's reordering): the first frame any socket is about to get is held back and queued
    /// after the next one that socket gets (a socket that never gets another one never gets the
    /// first). Behind the `testing` feature.
    #[cfg(any(test, feature = "testing"))]
    pub fn ws_swap_next_two(&self) {
        self.lock().knobs.ws_swap = WsSwap::Armed;
    }

    /// How many sockets are open on the server, through every connection (a socket whose session
    /// ended is not counted). Behind the `testing` feature.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn ws_open_count(&self) -> usize {
        let mut inner = self.lock();
        inner.reap_sockets();
        inner.sockets.len()
    }
}
