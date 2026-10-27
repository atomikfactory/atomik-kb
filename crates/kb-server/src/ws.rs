//! `GET /v2/ws` (M13 slice 3, ADR-0050, `docs/security/sharing.md` section 9): the realtime socket of a
//! device. The frames are `kb_sync::v2::ws`'s (documented there); what the server does with them:
//!
//! - **`subscribe`** replaces the connection's set. Each feed is checked against the access list (the
//!   account's own personal feed and the shared feeds it is a current member of); the rest are ignored
//!   without a word, so an unknown feed and a forbidden one look alike. Nothing is answered.
//! - **Items**: after an append commits (inside the store's lock, so notices leave in cursor order)
//!   the server publishes one notice per item to every connection. A connection that subscribed to the
//!   feed and whose account is a member after the append gets the entry frame (`changed` for an item
//!   over `INLINE_ENTRY_MAX_BYTES`), the appender's own connection included. A member the append
//!   removed gets the item that removed it (it may read the feed up to it) and its subscription ends:
//!   nothing after it is ever sent.
//! - **Lag**: a connection that fell behind the notice channel lost notices. It re-checks its
//!   subscriptions and is told the newest cursor of each feed that is left, as `changed` frames.
//! - **Presence**: a binary frame `2 | feed | envelope` is relayed, byte for byte, to the other
//!   connections subscribed to the feed, and stored nowhere. It is refused (dropped; the socket stays
//!   open and nothing is answered) when it is over 4 KiB, does not parse, names another feed in its
//!   header than the frame's or another account than the session's, is for a feed the sending
//!   connection is not subscribed to (the personal feed included: presence is for shared feeds), or
//!   exceeds `PRESENCE_FRAMES_PER_SECOND` on the connection (every binary frame counts, refused or not).
//!   A message over `CLIENT_MESSAGE_MAX_BYTES` ends the connection, and so does a connection that has
//!   a thousand frames refused or dropped within a minute.
//! - The server pings every 30 s and closes a connection that stays silent (no pong, no frame) for
//!   two intervals, and closes it when the session expires, or when a frame it has for the client
//!   cannot be written for 30 s (the client is not reading).
//! - **Revocation**: when an append to an account's device log makes a device inactive (`DeviceRevoked`,
//!   or any entry after which the verified log no longer lists it as active), the store deletes that
//!   device's sessions and challenges in the same transaction and, still under its lock, tells every
//!   connection: the device's end before anything else is sent (entries and presence alike). It cannot
//!   reconnect: the handshake authenticates the token against the session table and the log on every
//!   connection, and a new sign-in is refused for an inactive device.
//!
//! The log holds counts only, once, when a connection closes: never a feed, an account, a device or
//! a frame (the request's own span carries the 8-character prefixes of the account and device that
//! authenticated, as for every route).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use kb_sync::v2::{
    ClientFrame, FrameLimiter, InlineEntry, decode_client_binary, decode_client_text,
    encode_changed, encode_entry_frame, peek_presence,
};
use kb_types::sharing::INLINE_ENTRY_MAX_BYTES;
use kb_types::{AccountId, DeviceId, FeedId};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::{self, Receiver};
use tokio::time::Instant;

use crate::api::AppState;

/// Keepalive interval.
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(30);
/// Silence after which the connection is considered dead.
const IDLE_LIMIT: Duration = Duration::from_secs(75);
/// How many notices a channel holds for a slow connection before it is told where the feeds stand
/// instead.
const NOTICE_CAPACITY: usize = 1024;
/// `subscribe` frames a connection may send per second (each costs a few lookups).
const SUBSCRIBES_PER_SECOND: u32 = 10;
/// Frames refused or dropped per minute after which a connection is closed.
const ABUSE_FRAMES_PER_MINUTE: u32 = 1000;
/// How long a frame may wait to be written to a client that does not read.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// A feed item that was just stored, for the connections to deliver.
pub(crate) struct ItemNotice {
    feed: FeedId,
    cursor: u64,
    /// The accounts that are members of the feed after the item.
    members: Vec<AccountId>,
    /// The accounts the item removed from the members, with the cursor of the entry that did.
    ended: Vec<(AccountId, u64)>,
    /// The binary entry frame; `None` for an item over `INLINE_ENTRY_MAX_BYTES` (`changed` goes
    /// instead).
    frame: Option<Bytes>,
}

/// A presence frame a connection accepted, for the others to relay.
pub(crate) struct PresenceNotice {
    feed: FeedId,
    /// The connection it came from (which does not get it back).
    from: u64,
    /// The frame exactly as the client sent it.
    frame: Bytes,
}

/// Where an item landed in its feed: what the entry frame carries besides the bytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Position {
    pub(crate) feed: FeedId,
    /// The cursor of the item before it in the feed (0 when it is the first).
    pub(crate) prev_cursor: u64,
    pub(crate) cursor: u64,
    /// The feed's chain hash after it.
    pub(crate) chain_after: [u8; 32],
}

/// The channels the connections listen on, and the counters tests read. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Hub {
    items: broadcast::Sender<Arc<ItemNotice>>,
    presence: broadcast::Sender<Arc<PresenceNotice>>,
    /// Devices an account's log revoked: their connections end.
    revoked: broadcast::Sender<(AccountId, DeviceId)>,
    next_connection: Arc<AtomicU64>,
    subscribes: Arc<AtomicU64>,
}

impl Hub {
    pub(crate) fn new() -> Self {
        Self {
            items: broadcast::channel(NOTICE_CAPACITY).0,
            presence: broadcast::channel(NOTICE_CAPACITY).0,
            revoked: broadcast::channel(NOTICE_CAPACITY).0,
            next_connection: Arc::new(AtomicU64::new(0)),
            subscribes: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Publishes a stored item: `members` are the accounts that get it, `ended` the accounts it
    /// removed (see [`ItemNotice`]). Called while the store's lock is held, after the commit, so
    /// notices leave in the order of the cursors. Nobody listening is fine.
    pub(crate) fn publish_item(
        &self,
        position: Position,
        members: Vec<AccountId>,
        ended: Vec<(AccountId, u64)>,
        bytes: &[u8],
    ) {
        let frame = (bytes.len() <= INLINE_ENTRY_MAX_BYTES)
            .then(|| {
                encode_entry_frame(&InlineEntry {
                    feed: position.feed,
                    prev_cursor: position.prev_cursor,
                    cursor: position.cursor,
                    chain_after: position.chain_after,
                    entry: bytes.to_vec(),
                })
                .ok()
            })
            .flatten()
            .map(Bytes::from);
        let _ = self.items.send(Arc::new(ItemNotice {
            feed: position.feed,
            cursor: position.cursor,
            members,
            ended,
            frame,
        }));
    }

    /// `subscribe` frames handled so far, accepted or not (the tests wait on it: there is no
    /// acknowledgement on the wire).
    pub(crate) fn subscribes_handled(&self) -> u64 {
        self.subscribes.load(Ordering::SeqCst)
    }

    /// Publishes `count` notices nobody wants, to make slow connections lag (tests).
    pub(crate) fn flood(&self, count: usize) {
        for _ in 0..count {
            let _ = self.items.send(Arc::new(ItemNotice {
                feed: FeedId::from_bytes([0; 16]),
                cursor: 0,
                members: Vec::new(),
                ended: Vec::new(),
                frame: None,
            }));
        }
    }

    /// Tells the connections that `device` of `account` is not active in the account's log any more:
    /// theirs end at once, wherever they are (and with them their share of the presence relay). Called
    /// while the store's lock is held, after the commit that revoked it.
    pub(crate) fn publish_revoked(&self, account: AccountId, device: DeviceId) {
        let _ = self.revoked.send((account, device));
    }

    /// The receivers of a new connection, taken before the upgrade is answered so that nothing
    /// published during the handshake is missed.
    pub(crate) fn listen(&self) -> Listening {
        Listening {
            items: self.items.subscribe(),
            presence: self.presence.subscribe(),
            revoked: self.revoked.subscribe(),
        }
    }
}

/// What a connection listens to.
pub(crate) struct Listening {
    items: Receiver<Arc<ItemNotice>>,
    presence: Receiver<Arc<PresenceNotice>>,
    revoked: Receiver<(AccountId, DeviceId)>,
}

/// What a connection does with an item notice for a feed it subscribed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    /// Send it.
    Send,
    /// Send it and end the subscription: it is the item that removed the account.
    SendAndEnd,
    /// Do not send it and end the subscription: the account is no member of the feed any more.
    End,
}

fn delivery(notice: &ItemNotice, account: AccountId) -> Delivery {
    if notice.members.contains(&account) {
        return Delivery::Send;
    }
    match notice.ended.iter().find(|(ended, _)| *ended == account) {
        Some((_, end)) if notice.cursor <= *end => Delivery::SendAndEnd,
        _ => Delivery::End,
    }
}

/// The connection is over.
struct Closed;

/// What happened on a connection, for the one log line when it closes.
#[derive(Default)]
struct Counts {
    entries: u64,
    relayed: u64,
    dropped: u64,
    refused: u64,
    lagged: u64,
}

struct Connection {
    socket: WebSocket,
    state: Arc<AppState>,
    account: AccountId,
    device: DeviceId,
    id: u64,
    /// The subscription: each feed, and whether it is the account's personal feed.
    feeds: HashMap<FeedId, bool>,
    presence_rate: FrameLimiter,
    subscribe_rate: FrameLimiter,
    abuse: FrameLimiter,
    started: Instant,
    last_heard: Instant,
    counts: Counts,
}

impl Connection {
    /// Milliseconds on the connection's own monotonic clock (the rate limits count real time).
    fn now_ms(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    /// A frame was refused (`dropped`: for the rate) and the socket stays open, unless it is a lot.
    fn bad(&mut self, dropped: bool) -> Result<(), Closed> {
        if dropped {
            self.counts.dropped += 1;
        } else {
            self.counts.refused += 1;
        }
        let now = self.now_ms();
        if self.abuse.allow(now) {
            Ok(())
        } else {
            Err(Closed)
        }
    }

    /// Sends a frame; a client that does not read for `SEND_TIMEOUT` ends the connection (the task
    /// would otherwise wait on it for ever and neither ping nor see the session expire).
    async fn send(&mut self, message: Message) -> Result<(), Closed> {
        match tokio::time::timeout(SEND_TIMEOUT, self.socket.send(message)).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(Closed),
        }
    }

    async fn send_changed(&mut self, feed: FeedId, cursor: u64) -> Result<(), Closed> {
        match encode_changed(feed, cursor) {
            Ok(text) => self.send(Message::Text(text.into())).await,
            Err(_) => Ok(()),
        }
    }

    async fn on_item(&mut self, notice: &ItemNotice) -> Result<(), Closed> {
        if !self.feeds.contains_key(&notice.feed) {
            return Ok(());
        }
        let what = delivery(notice, self.account);
        if what != Delivery::Send {
            self.feeds.remove(&notice.feed);
        }
        if what == Delivery::End {
            return Ok(());
        }
        self.counts.entries += 1;
        match &notice.frame {
            Some(frame) => self.send(Message::Binary(frame.clone())).await,
            None => self.send_changed(notice.feed, notice.cursor).await,
        }
    }

    /// The channel dropped notices this connection had not read: some membership may have ended
    /// without it noticing, and some items were not pushed. Check the subscription again and tell
    /// the client where each feed stands.
    async fn on_lag(&mut self) -> Result<(), Closed> {
        self.counts.lagged += 1;
        let account = self.account;
        let now = self.state.now_ms();
        let feeds: Vec<FeedId> = self.feeds.keys().copied().collect();
        let checked = self
            .state
            .db(move |store| {
                let kept = store.subscribable(account, &feeds, now)?;
                let ids: Vec<FeedId> = kept.iter().map(|(feed, _)| *feed).collect();
                let cursors = store.feed_head_cursors(&ids)?;
                Ok((kept, cursors))
            })
            .await;
        let Ok((kept, cursors)) = checked else {
            return Ok(());
        };
        self.feeds = kept.into_iter().collect();
        for (feed, cursor) in cursors {
            self.send_changed(feed, cursor).await?;
        }
        Ok(())
    }

    /// The log of this connection's account revoked a device: this connection ends if it is that one.
    fn on_revoked(&self, account: AccountId, device: DeviceId) -> Result<(), Closed> {
        if account == self.account && device == self.device {
            Err(Closed)
        } else {
            Ok(())
        }
    }

    /// Revocation notices were lost (the channel lagged): ask the store whether this device is
    /// still active.
    async fn recheck_device(&mut self) -> Result<(), Closed> {
        let (account, device) = (self.account, self.device);
        let active = self
            .state
            .db(move |store| {
                Ok(store
                    .log_state(account)?
                    .is_some_and(|state| state.active(device).is_some()))
            })
            .await;
        if matches!(active, Ok(false)) {
            Err(Closed)
        } else {
            Ok(())
        }
    }

    async fn on_presence(&mut self, notice: &PresenceNotice) -> Result<(), Closed> {
        if notice.from == self.id || self.feeds.get(&notice.feed) != Some(&false) {
            return Ok(());
        }
        self.send(Message::Binary(notice.frame.clone())).await
    }

    async fn on_message(&mut self, message: Message) -> Result<(), Closed> {
        match message {
            Message::Text(text) => self.on_text(text.as_str()).await,
            Message::Binary(bytes) => self.on_binary(bytes),
            // Pings are answered by the protocol layer; a pong only proves the client is there.
            Message::Ping(_) | Message::Pong(_) => Ok(()),
            Message::Close(_) => Err(Closed),
        }
    }

    async fn on_text(&mut self, text: &str) -> Result<(), Closed> {
        let now = self.now_ms();
        if !self.subscribe_rate.allow(now) {
            return self.bad(true);
        }
        let Ok(ClientFrame::Subscribe(feeds)) = decode_client_text(text) else {
            return self.bad(false);
        };
        let account = self.account;
        let now = self.state.now_ms();
        if let Ok(kept) = self
            .state
            .db(move |store| store.subscribable(account, &feeds, now))
            .await
        {
            self.feeds = kept.into_iter().collect();
        }
        self.state.hub.subscribes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn on_binary(&mut self, bytes: Bytes) -> Result<(), Closed> {
        let now = self.now_ms();
        if !self.presence_rate.allow(now) {
            return self.bad(true);
        }
        let Ok(ClientFrame::Presence(frame)) = decode_client_binary(&bytes) else {
            return self.bad(false);
        };
        let Ok(header) = peek_presence(&frame.envelope) else {
            return self.bad(false);
        };
        let shared_and_subscribed = self.feeds.get(&frame.feed) == Some(&false);
        if header.feed != frame.feed || header.account_id != self.account || !shared_and_subscribed
        {
            return self.bad(false);
        }
        let _ = self.state.hub.presence.send(Arc::new(PresenceNotice {
            feed: frame.feed,
            from: self.id,
            frame: bytes,
        }));
        self.counts.relayed += 1;
        Ok(())
    }
}

/// Serves one connection until the client leaves, goes silent, the session expires or
/// the server shuts down.
pub(crate) async fn serve(
    socket: WebSocket,
    mut listen: Listening,
    state: Arc<AppState>,
    account: AccountId,
    device: DeviceId,
    session_expires_at: i64,
) {
    let remaining = session_expires_at.saturating_sub(state.now_ms()).max(0);
    let deadline = Instant::now() + Duration::from_millis(u64::try_from(remaining).unwrap_or(0));
    let mut ping = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let id = state.hub.next_connection.fetch_add(1, Ordering::SeqCst);
    let mut connection = Connection {
        socket,
        state,
        account,
        device,
        id,
        feeds: HashMap::new(),
        presence_rate: FrameLimiter::presence(),
        subscribe_rate: FrameLimiter::new(SUBSCRIBES_PER_SECOND, 1_000),
        abuse: FrameLimiter::new(ABUSE_FRAMES_PER_MINUTE, 60_000),
        started: Instant::now(),
        last_heard: Instant::now(),
        counts: Counts::default(),
    };
    let Listening {
        items,
        presence,
        revoked,
    } = &mut listen;
    loop {
        let step = tokio::select! {
            biased;
            // A revocation comes before anything else: nothing more is sent to a device the log
            // revoked, not an entry and not a presence frame.
            received = revoked.recv() => match received {
                Ok((account, device)) => connection.on_revoked(account, device),
                Err(RecvError::Lagged(_)) => connection.recheck_device().await,
                Err(RecvError::Closed) => Err(Closed),
            },
            received = items.recv() => match received {
                Ok(notice) => connection.on_item(&notice).await,
                Err(RecvError::Lagged(_)) => connection.on_lag().await,
                Err(RecvError::Closed) => Err(Closed),
            },
            received = presence.recv() => match received {
                Ok(notice) => connection.on_presence(&notice).await,
                // Presence is best effort: frames that were lost are simply gone.
                Err(RecvError::Lagged(_)) => Ok(()),
                Err(RecvError::Closed) => Err(Closed),
            },
            incoming = connection.socket.recv() => match incoming {
                None | Some(Err(_)) => Err(Closed),
                Some(Ok(message)) => {
                    connection.last_heard = Instant::now();
                    connection.on_message(message).await
                }
            },
            _ = ping.tick() => {
                if connection.last_heard.elapsed() > IDLE_LIMIT {
                    Err(Closed)
                } else {
                    connection.send(Message::Ping(Bytes::new())).await
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                let _ = connection.socket.send(Message::Close(None)).await;
                Err(Closed)
            }
        };
        if step.is_err() {
            break;
        }
    }
    let counts = &connection.counts;
    tracing::info!(
        entries = counts.entries,
        presence_relayed = counts.relayed,
        dropped = counts.dropped,
        refused = counts.refused,
        lagged = counts.lagged,
        "realtime connection closed"
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use kb_sync::v2::ServerFrame;
    use kb_sync::v2::decode_server_binary;

    fn account(n: u8) -> AccountId {
        AccountId::from_bytes([n; 16])
    }

    fn notice(members: &[u8], ended: &[(u8, u64)], cursor: u64) -> ItemNotice {
        ItemNotice {
            feed: FeedId::from_bytes([7; 16]),
            cursor,
            members: members.iter().map(|n| account(*n)).collect(),
            ended: ended.iter().map(|(n, c)| (account(*n), *c)).collect(),
            frame: None,
        }
    }

    #[test]
    fn a_member_gets_the_item_and_a_stranger_loses_the_subscription() {
        let item = notice(&[1, 2], &[], 10);
        assert_eq!(delivery(&item, account(1)), Delivery::Send);
        assert_eq!(delivery(&item, account(2)), Delivery::Send);
        assert_eq!(delivery(&item, account(3)), Delivery::End);
    }

    #[test]
    fn a_removed_member_gets_the_item_that_removed_it_and_nothing_after() {
        // Account 3 was removed at cursor 10 by a batch of two entries (cursors 10 and 11).
        let remove = notice(&[1, 2], &[(3, 10)], 10);
        let rekey = notice(&[1, 2], &[(3, 10)], 11);
        assert_eq!(delivery(&remove, account(3)), Delivery::SendAndEnd);
        assert_eq!(delivery(&rekey, account(3)), Delivery::End);
        assert_eq!(delivery(&remove, account(1)), Delivery::Send);
        // A member that rejoined in the same batch is a member.
        assert_eq!(
            delivery(&notice(&[3], &[(3, 10)], 11), account(3)),
            Delivery::Send
        );
    }

    #[tokio::test]
    async fn an_item_is_published_as_the_entry_frame_or_not_inline() {
        let hub = Hub::new();
        let mut items = hub.listen().items;
        let position = Position {
            feed: FeedId::from_bytes([7; 16]),
            prev_cursor: 4,
            cursor: 9,
            chain_after: [3; 32],
        };
        hub.publish_item(position, vec![account(1)], Vec::new(), b"small entry");
        let small = items.recv().await.unwrap();
        let ServerFrame::Entry(entry) =
            decode_server_binary(small.frame.as_ref().unwrap()).unwrap()
        else {
            panic!("an entry frame");
        };
        assert_eq!(
            (
                entry.feed,
                entry.prev_cursor,
                entry.cursor,
                entry.chain_after,
                entry.entry.as_slice()
            ),
            (position.feed, 4, 9, [3; 32], &b"small entry"[..])
        );
        assert_eq!((small.cursor, small.members.clone()), (9, vec![account(1)]));
        // An item over the inline limit has no frame: the connection sends `changed`.
        hub.publish_item(
            position,
            vec![account(1)],
            Vec::new(),
            &vec![0u8; INLINE_ENTRY_MAX_BYTES + 1],
        );
        assert!(items.recv().await.unwrap().frame.is_none());
        // The limit itself is inline.
        hub.publish_item(
            position,
            vec![account(1)],
            Vec::new(),
            &vec![0u8; INLINE_ENTRY_MAX_BYTES],
        );
        assert!(items.recv().await.unwrap().frame.is_some());
    }

    #[tokio::test]
    async fn publishing_with_nobody_listening_is_fine_and_a_flood_lags_a_slow_receiver() {
        let hub = Hub::new();
        hub.flood(3);
        let mut items = hub.listen().items;
        hub.flood(NOTICE_CAPACITY + 5);
        assert!(matches!(items.recv().await, Err(RecvError::Lagged(_))));
        // Presence has a channel of its own: a flood of items does not make presence lag.
        let mut presence = hub.listen().presence;
        hub.flood(NOTICE_CAPACITY + 5);
        assert!(presence.try_recv().is_err());
    }
}
