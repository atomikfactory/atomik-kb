//! The realtime checks of the conformance suite (M13 slice 3, ADR-0050): what `GET /v2/ws` does,
//! for [`MemoryRemote`]'s socket and the real server over a real WebSocket alike.
//!
//! The remote under test is driven twice: through [`RemoteStore`] (`remote`, as the other checks do:
//! one handle that signs in as whoever acts next) and through a [`WsHarness`] (`ws`), which opens
//! sockets **as a person** (each on its own session, so they keep receiving while the handle signs
//! in as someone else) and can let time pass. A socket is a [`WsConnection`]: `subscribe`,
//! `send_presence`, raw sends for the malformed cases, and `recv` with a wait (a real socket waits;
//! the reference remote's answers at once).
//!
//! What the checks fix, which `MemoryRemote` and `kb-server` both do:
//!
//! - [`check_ws_inline_entries`]: a subscription is checked against the access list and *replaces* the
//!   set; every item appended to a subscribed feed (the personal feed too) arrives as an entry frame
//!   with the position (`prev_cursor` is the feed's previous item, whoever the receiver is), the
//!   appender's own socket included, in the feed's order, byte for byte what a pull returns; a stranger
//!   hears nothing and a duplicate append pushes nothing;
//! - [`check_ws_large_entries_are_announced`]: an item over `INLINE_ENTRY_MAX_BYTES` arrives as `changed`;
//! - [`check_ws_frames_follow_the_feeds_order`]: appends by several members arrive everywhere in cursor
//!   order with a contiguous chain of `prev_cursor`s;
//! - [`check_ws_removed_member_stops_at_its_remove`]: membership entries are pushed too; a removed or
//!   departed member's socket gets that entry and nothing after it, and its presence stops both ways;
//! - [`check_ws_presence_relay`]: presence goes to the other subscribed sockets of the feed (Viewers
//!   send too, the sender's own device-mates receive), never to the sender's socket and never to a
//!   stranger, and is never stored;
//! - [`check_ws_presence_refusals`]: a spoofed account, another feed than the frame's, a feed the socket
//!   is not subscribed to, the personal feed, garbage and a frame that is not a presence frame are
//!   dropped and the socket stays open;
//! - [`check_ws_presence_rate_limit`]: the excess over `PRESENCE_FRAMES_PER_SECOND` per connection is
//!   dropped, and frames pass again a second later;
//! - [`check_ws_revoked_device_is_closed_and_cannot_reconnect`]: a device the account's log revokes loses
//!   its socket at once, cannot sign in again to open another, and is out of the presence relay.
//!
//! [`run_all_ws`] runs them all. They are not part of [`super::run_all_v2`], which takes a remote
//! without sockets.

#![allow(clippy::panic)]

use kb_types::sharing::PRESENCE_FRAMES_PER_SECOND;
use kb_types::testing::SeededRng;
use kb_types::{FeedId, MemberRole};

use super::kit::{Person, sealed_entry};
use super::{
    Fixture, enroll_n, feed_tip, found, holds, invite_and_join, login, ok, post_ok, read_feed,
};
use crate::SyncError;
use crate::v2::memory::{MemoryRemote, MemorySocket};
use crate::v2::ws::{
    ClientFrame, InlineEntry, PresenceFrame, ServerFrame, decode_client_binary, decode_client_text,
    peek_presence,
};
use crate::v2::{ItemKind, RemoteStore, feed_kind};

/// How long a frame that must arrive may take.
const WAIT_MS: u64 = 5_000;
/// How long to listen for a frame that must not arrive.
const QUIET_MS: u64 = 300;

/// A realtime socket of the remote under test, opened by [`WsHarness::open_socket`].
#[allow(async_fn_in_trait)]
pub trait WsConnection {
    /// Sends `subscribe` and answers once the remote has applied it (a server applies a subscription
    /// asynchronously: its harness waits until it has).
    async fn subscribe(&self, feeds: &[FeedId]) -> Result<(), SyncError>;

    /// Sends a presence frame. A real server answers nothing, so only a reference remote's answer
    /// can be an error: the checks look at what other sockets receive, never at this result for a
    /// frame that is meant to be refused.
    async fn send_presence(&self, frame: &PresenceFrame) -> Result<(), SyncError>;

    /// Sends a binary frame as it is (garbage, an oversize frame, a frame only a server sends).
    async fn send_binary(&self, bytes: &[u8]) -> Result<(), SyncError>;

    /// Sends a text frame as it is.
    async fn send_text(&self, text: &str) -> Result<(), SyncError>;

    /// The next frame, waiting up to about `wait_ms` milliseconds for it; `None` when none came.
    async fn recv(&self, wait_ms: u64) -> Option<ServerFrame>;

    /// Whether the remote closes this socket within about `wait_ms` milliseconds (a real socket waits
    /// for the close; the reference remote's answers at once).
    async fn is_closed(&self, wait_ms: u64) -> bool;
}

/// What the realtime checks need besides [`RemoteStore`].
#[allow(async_fn_in_trait)]
pub trait WsHarness {
    /// The socket type.
    type Socket: WsConnection;

    /// Opens a socket for `person`'s first active device, on a session of its own: the handle the
    /// checks drive the remote through is not touched, and the socket keeps receiving when that
    /// handle signs in as someone else. The person's account must exist.
    async fn open_socket(&self, person: &Person) -> Result<Self::Socket, SyncError> {
        self.open_device_socket(person, person.active_device())
            .await
    }

    /// [`Self::open_socket`] for `person`'s device number `device`; a device that is not active in the
    /// account's log is refused (`Unauthorized`): it cannot sign in, so it cannot reconnect.
    async fn open_device_socket(
        &self,
        person: &Person,
        device: usize,
    ) -> Result<Self::Socket, SyncError>;

    /// Lets `ms` milliseconds pass for the remote's rate limiter (the reference remote moves its
    /// clock, a real server waits).
    async fn wait_ms(&self, ms: u64);
}

impl WsConnection for MemorySocket {
    async fn subscribe(&self, feeds: &[FeedId]) -> Result<(), SyncError> {
        MemorySocket::subscribe(self, feeds)
    }

    async fn send_presence(&self, frame: &PresenceFrame) -> Result<(), SyncError> {
        MemorySocket::send_presence(self, frame)
    }

    async fn send_binary(&self, bytes: &[u8]) -> Result<(), SyncError> {
        match decode_client_binary(bytes) {
            Ok(ClientFrame::Presence(frame)) => MemorySocket::send_presence(self, &frame),
            _ => Ok(()),
        }
    }

    async fn send_text(&self, text: &str) -> Result<(), SyncError> {
        match decode_client_text(text) {
            Ok(ClientFrame::Subscribe(feeds)) => MemorySocket::subscribe(self, &feeds),
            _ => Ok(()),
        }
    }

    async fn recv(&self, _wait_ms: u64) -> Option<ServerFrame> {
        MemorySocket::recv(self)
    }

    async fn is_closed(&self, _wait_ms: u64) -> bool {
        !MemorySocket::is_open(self)
    }
}

impl WsHarness for MemoryRemote {
    type Socket = MemorySocket;

    async fn open_device_socket(
        &self,
        person: &Person,
        device: usize,
    ) -> Result<MemorySocket, SyncError> {
        let connection = self.clone();
        connection.authenticate_sync(
            person.account,
            person.device_id(device),
            &person.signer(device),
        )?;
        connection.ws_open()
    }

    async fn wait_ms(&self, ms: u64) {
        self.advance_ms(i64::try_from(ms).unwrap_or(i64::MAX));
    }
}

// ----- helpers ------------------------------------------------------------------------------

fn kind_of(frame: &ServerFrame) -> &'static str {
    match frame {
        ServerFrame::Changed { .. } => "a changed frame",
        ServerFrame::Entry(_) => "an entry frame",
        ServerFrame::Presence(_) => "a presence frame",
    }
}

/// The next frame, which must be an entry frame.
async fn entry_of<S: WsConnection>(socket: &S, what: &str) -> InlineEntry {
    match socket.recv(WAIT_MS).await {
        Some(ServerFrame::Entry(entry)) => entry,
        Some(other) => panic!(
            "conformance: {what}: expected an entry frame, got {}",
            kind_of(&other)
        ),
        None => panic!("conformance: {what}: no frame arrived"),
    }
}

/// The next frame, which must be a presence frame.
async fn presence_of<S: WsConnection>(socket: &S, what: &str) -> PresenceFrame {
    match socket.recv(WAIT_MS).await {
        Some(ServerFrame::Presence(frame)) => frame,
        Some(other) => panic!(
            "conformance: {what}: expected a presence frame, got {}",
            kind_of(&other)
        ),
        None => panic!("conformance: {what}: no frame arrived"),
    }
}

/// The socket receives nothing for a while.
async fn quiet<S: WsConnection>(socket: &S, what: &str) {
    if let Some(frame) = socket.recv(QUIET_MS).await {
        panic!("conformance: {what}: got {}", kind_of(&frame));
    }
}

/// A socket of `person` subscribed to `feeds`.
async fn subscribed<H: WsHarness>(
    ws: &H,
    person: &Person,
    feeds: &[FeedId],
    what: &str,
) -> H::Socket {
    let socket = ok(ws.open_socket(person).await, what);
    ok(socket.subscribe(feeds).await, what);
    socket
}

// ----- the checks ---------------------------------------------------------------------------

/// Runs every realtime check against `remote` and `ws`.
pub async fn run_all_ws<R: RemoteStore, H: WsHarness>(remote: &R, ws: &H, fx: &Fixture) {
    check_ws_inline_entries(remote, ws, fx).await;
    check_ws_large_entries_are_announced(remote, ws, fx).await;
    check_ws_frames_follow_the_feeds_order(remote, ws, fx).await;
    check_ws_removed_member_stops_at_its_remove(remote, ws, fx).await;
    check_ws_presence_relay(remote, ws, fx).await;
    check_ws_presence_refusals(remote, ws, fx).await;
    check_ws_presence_rate_limit(remote, ws, fx).await;
    check_ws_revoked_device_is_closed_and_cannot_reconnect(remote, ws, fx).await;
}

/// Subscriptions and inline entries: see the [module documentation](self).
pub async fn check_ws_inline_entries<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob, erin] = enroll_n::<R, 3>(remote, fx).await;
    let mut space = fx.space("Inline");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    let alice_home = FeedId::from_account(alice.account);
    let bob_home = FeedId::from_account(bob.account);
    let alice_ws = subscribed(ws, &alice, &[feed, alice_home], "alice subscribes").await;
    let bob_ws = subscribed(ws, &bob, &[feed, bob_home], "bob subscribes").await;
    // A stranger's subscription to the feed, to another account's personal feed and to a feed that
    // does not exist is ignored, and not an error.
    let erin_ws = subscribed(
        ws,
        &erin,
        &[feed, alice_home, space.random_feed()],
        "a stranger subscribes",
    )
    .await;

    // The first append: both members' sockets, the appender's own included, get the entry with the
    // position of the item before it (the last membership entry).
    login(remote, &alice).await;
    let (tip, _) = feed_tip(remote, feed).await;
    let doc = fx.doc();
    let first = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"first");
    let (cursor, chain) = ok(remote.append(feed, &first).await, "append");
    for (who, socket) in [("alice", &alice_ws), ("bob", &bob_ws)] {
        assert_eq!(
            entry_of(socket, &format!("{who} gets the entry")).await,
            InlineEntry {
                feed,
                prev_cursor: tip,
                cursor,
                chain_after: chain,
                entry: first.clone(),
            },
            "conformance: the entry frame of {who} carries the item and its position"
        );
    }
    quiet(&erin_ws, "a stranger hears nothing of the feed").await;

    // Appending the same entry again is a duplicate: nothing is pushed.
    let again = ok(remote.append(feed, &first).await, "append again");
    assert_eq!(
        again,
        (cursor, chain),
        "conformance: a duplicate answers the stored position"
    );
    quiet(&bob_ws, "a duplicate append pushes nothing").await;

    // Two entries in a row from Bob: Alice's socket and Bob's own get them in order, and each
    // frame's previous cursor is the cursor of the item before it.
    login(remote, &bob).await;
    let second = space.doc_entry(&bob, 0, doc, 1, space.epoch, b"second");
    let (second_cursor, second_chain) = ok(remote.append(feed, &second).await, "append");
    let third = space.doc_entry(&bob, 0, doc, 2, space.epoch, b"third");
    let (third_cursor, third_chain) = ok(remote.append(feed, &third).await, "append");
    let mut frames = Vec::new();
    for (who, socket) in [("alice", &alice_ws), ("bob", &bob_ws)] {
        let one = entry_of(socket, &format!("{who} gets the second entry")).await;
        let two = entry_of(socket, &format!("{who} gets the third entry")).await;
        assert_eq!(
            (one.prev_cursor, one.cursor, one.chain_after),
            (cursor, second_cursor, second_chain),
            "conformance: the second entry follows the first"
        );
        assert_eq!(
            (two.prev_cursor, two.cursor, two.chain_after),
            (second_cursor, third_cursor, third_chain),
            "conformance: the third entry follows the second"
        );
        assert_eq!(
            (one.entry.as_slice(), two.entry.as_slice()),
            (&second[..], &third[..])
        );
        frames.extend([one, two]);
    }

    // What the sockets got is what a pull returns, byte for byte.
    let items = read_feed(remote, feed).await;
    for frame in &frames {
        assert!(
            items
                .iter()
                .any(|(at, bytes)| *at == frame.cursor && *bytes == frame.entry),
            "conformance: an inline entry equals the pulled item at its cursor"
        );
    }

    // The personal feed is subscribed like any other and is the account's alone.
    login(remote, &alice).await;
    let home_doc = fx.doc();
    let personal = sealed_entry(
        alice_home,
        &alice,
        0,
        home_doc,
        1,
        0,
        b"personal",
        &SeededRng::new(fx.seed() ^ 0x9e57),
    );
    let (home_cursor, home_chain) = ok(remote.append(alice_home, &personal).await, "append home");
    let pushed = entry_of(&alice_ws, "alice gets her personal entry").await;
    assert_eq!(
        (
            pushed.feed,
            pushed.cursor,
            pushed.chain_after,
            pushed.entry.as_slice()
        ),
        (alice_home, home_cursor, home_chain, &personal[..])
    );
    quiet(
        &bob_ws,
        "a member does not hear another account's personal feed",
    )
    .await;
    quiet(&erin_ws, "a stranger does not hear a personal feed").await;

    // A subscription replaces the set: with the personal feed only, the shared feed goes quiet.
    ok(alice_ws.subscribe(&[alice_home]).await, "resubscribe");
    login(remote, &bob).await;
    let fourth = space.doc_entry(&bob, 0, doc, 3, space.epoch, b"fourth");
    let (fourth_cursor, _) = ok(remote.append(feed, &fourth).await, "append");
    assert_eq!(
        entry_of(&bob_ws, "bob gets his own entry").await.cursor,
        fourth_cursor
    );
    quiet(&alice_ws, "a feed that is not in the set is not pushed").await;
    // An empty set unsubscribes from everything.
    ok(alice_ws.subscribe(&[]).await, "unsubscribe");
    login(remote, &alice).await;
    let more = sealed_entry(
        alice_home,
        &alice,
        0,
        home_doc,
        2,
        0,
        b"more",
        &SeededRng::new(fx.seed() ^ 0x9e58),
    );
    ok(remote.append(alice_home, &more).await, "append home");
    quiet(&alice_ws, "an empty subscription hears nothing").await;
    // A socket that subscribes late gets the next entry with the true previous cursor: the item
    // before it in the feed, not the last item this socket saw.
    ok(alice_ws.subscribe(&[feed]).await, "subscribe again");
    login(remote, &bob).await;
    let fifth = space.doc_entry(&bob, 0, doc, 4, space.epoch, b"fifth");
    let (fifth_cursor, _) = ok(remote.append(feed, &fifth).await, "append");
    let late = entry_of(&alice_ws, "alice gets the entry after resubscribing").await;
    assert_eq!(
        (late.prev_cursor, late.cursor),
        (fourth_cursor, fifth_cursor),
        "conformance: the previous cursor is the feed's, whoever the receiver is"
    );
}

/// An item over `INLINE_ENTRY_MAX_BYTES` is announced with `changed`.
pub async fn check_ws_large_entries_are_announced<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Large");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    let bob_ws = subscribed(ws, &bob, &[feed], "bob subscribes").await;
    let doc = fx.doc();
    login(remote, &alice).await;
    // A small entry is inline.
    let small = space.doc_entry(&alice, 0, doc, 1, space.epoch, &[7u8; 20_000]);
    assert!(small.len() <= kb_types::sharing::INLINE_ENTRY_MAX_BYTES);
    let (small_cursor, _) = ok(remote.append(feed, &small).await, "append");
    assert_eq!(
        entry_of(&bob_ws, "bob gets the small entry").await.cursor,
        small_cursor
    );
    // One padded to the next bucket does not fit: the frame is `changed` with its cursor.
    let large = space.doc_entry(&alice, 0, doc, 2, space.epoch, &[9u8; 40_000]);
    assert!(large.len() > kb_types::sharing::INLINE_ENTRY_MAX_BYTES);
    let (large_cursor, _) = ok(remote.append(feed, &large).await, "append");
    assert_eq!(
        bob_ws.recv(WAIT_MS).await,
        Some(ServerFrame::Changed {
            feed,
            cursor: large_cursor
        }),
        "conformance: an item over the inline limit is announced with changed"
    );
    // The next small one is inline again, and names the large one as the item before it.
    let next = space.doc_entry(&alice, 0, doc, 3, space.epoch, b"next");
    let (next_cursor, _) = ok(remote.append(feed, &next).await, "append");
    let pushed = entry_of(&bob_ws, "bob gets the entry after the large one").await;
    assert_eq!(
        (pushed.prev_cursor, pushed.cursor),
        (large_cursor, next_cursor)
    );
    // The pull still has the large item, which the chain covers.
    let (tip, _) = feed_tip(remote, feed).await;
    assert_eq!(tip, next_cursor);
    assert!(holds(&read_feed(remote, feed).await, &large));
}

/// Appends by several members arrive everywhere in the feed's order.
pub async fn check_ws_frames_follow_the_feeds_order<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob, carol] = enroll_n::<R, 3>(remote, fx).await;
    let mut space = fx.space("Order");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    invite_and_join(remote, fx, &mut space, &alice, &carol, MemberRole::Editor).await;
    let feed = space.feed;
    let sockets = [
        subscribed(ws, &alice, &[feed], "alice subscribes").await,
        subscribed(ws, &bob, &[feed], "bob subscribes").await,
        subscribed(ws, &carol, &[feed], "carol subscribes").await,
    ];
    login(remote, &alice).await;
    let (mut last, _) = feed_tip(remote, feed).await;
    let people = [&alice, &bob, &carol];
    let doc = fx.doc();
    let mut seqs = [0u64; 3];
    let mut expected = Vec::new();
    for round in 0..3usize {
        for (index, person) in people.iter().enumerate() {
            seqs[index] += 1;
            login(remote, person).await;
            let bytes =
                space.doc_entry(person, 0, doc, seqs[index], space.epoch, &[round as u8; 16]);
            let (cursor, chain) = ok(remote.append(feed, &bytes).await, "append");
            expected.push((last, cursor, chain, bytes));
            last = cursor;
        }
    }
    for (who, socket) in ["alice", "bob", "carol"].iter().zip(&sockets) {
        for (prev, cursor, chain, bytes) in &expected {
            let frame = entry_of(socket, &format!("{who} gets every entry")).await;
            assert_eq!(
                (
                    frame.prev_cursor,
                    frame.cursor,
                    frame.chain_after,
                    frame.entry.as_slice()
                ),
                (*prev, *cursor, *chain, bytes.as_slice()),
                "conformance: {who} gets the entries in the feed's order with a contiguous chain"
            );
        }
        quiet(socket, "nothing more than the appended entries").await;
    }
}

/// A removed or departed member's socket gets its `Remove` or `Leave` and nothing after it.
pub async fn check_ws_removed_member_stops_at_its_remove<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob, carol, dave] = enroll_n::<R, 4>(remote, fx).await;
    let mut space = fx.space("Stops");
    found(remote, &mut space, &alice).await;
    for joiner in [&bob, &carol, &dave] {
        invite_and_join(remote, fx, &mut space, &alice, joiner, MemberRole::Editor).await;
    }
    let feed = space.feed;
    let alice_ws = subscribed(ws, &alice, &[feed], "alice subscribes").await;
    let bob_ws = subscribed(ws, &bob, &[feed], "bob subscribes").await;
    let carol_ws = subscribed(ws, &carol, &[feed], "carol subscribes").await;
    let dave_ws = subscribed(ws, &dave, &[feed], "dave subscribes").await;

    // A membership entry is pushed like any other item, to every member.
    space.now = fx.now();
    let profile = space.profile(&carol, "Carol, renamed");
    post_ok(remote, &mut space, &carol, &profile, "the Profile").await;
    for (who, socket) in [
        ("alice", &alice_ws),
        ("bob", &bob_ws),
        ("carol", &carol_ws),
        ("dave", &dave_ws),
    ] {
        let pushed = entry_of(socket, &format!("{who} gets the membership entry")).await;
        assert_eq!(pushed.entry, profile.encode());
        assert_eq!(
            feed_kind(&pushed.entry),
            Ok(ItemKind::Membership),
            "conformance: a membership entry is relayed inline"
        );
    }

    // Alice removes Bob. Everyone, Bob included, gets the Remove; nothing follows for Bob.
    space.now = fx.now();
    let remove = space.remove(&alice, bob.account, &[&alice, &carol, &dave]);
    post_ok(remote, &mut space, &alice, &remove, "the Remove").await;
    login(remote, &alice).await;
    let remove_cursor = ok(
        remote
            .membership_get(feed, Some(remove.header.seq - 1))
            .await,
        "membership_get",
    )
    .entries[0]
        .cursor;
    for (who, socket) in [
        ("alice", &alice_ws),
        ("bob", &bob_ws),
        ("carol", &carol_ws),
        ("dave", &dave_ws),
    ] {
        let pushed = entry_of(socket, &format!("{who} gets the Remove")).await;
        assert_eq!(
            (pushed.cursor, pushed.entry.as_slice()),
            (remove_cursor, remove.encode().as_slice()),
            "conformance: the Remove goes to every socket, the removed member's too"
        );
    }
    let doc = fx.doc();
    let after = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"after the removal");
    let (after_cursor, _) = ok(remote.append(feed, &after).await, "append after the Remove");
    for (who, socket) in [
        ("alice", &alice_ws),
        ("carol", &carol_ws),
        ("dave", &dave_ws),
    ] {
        let pushed = entry_of(socket, &format!("{who} gets the entry after the Remove")).await;
        assert_eq!(pushed.cursor, after_cursor);
        assert_eq!(pushed.prev_cursor, remove_cursor);
    }
    quiet(
        &bob_ws,
        "a removed member's socket gets nothing after its Remove",
    )
    .await;

    // Presence stops both ways: Bob's frame goes nowhere, and Carol's does not reach Bob.
    let from_bob = space.presence_frame(&bob, 0, space.epoch - 1, 1, b"bob");
    let _ = bob_ws.send_presence(&from_bob).await;
    let from_carol = space.presence_frame(&carol, 0, space.epoch, 1, b"carol");
    ok(
        carol_ws.send_presence(&from_carol).await,
        "carol sends presence",
    );
    assert_eq!(
        presence_of(&alice_ws, "alice gets Carol's presence, not Bob's").await,
        from_carol
    );
    assert_eq!(
        presence_of(&dave_ws, "dave gets Carol's presence").await,
        from_carol
    );
    quiet(&bob_ws, "a removed member gets no presence").await;
    // Subscribing again changes nothing: the feed is not Bob's to read.
    ok(
        bob_ws.subscribe(&[feed]).await,
        "a removed member's subscribe is not an error",
    );
    login(remote, &alice).await;
    let later = space.doc_entry(&alice, 0, doc, 2, space.epoch, b"later");
    let (later_cursor, _) = ok(remote.append(feed, &later).await, "append");
    assert_eq!(
        entry_of(&carol_ws, "carol gets the later entry")
            .await
            .cursor,
        later_cursor
    );
    quiet(&bob_ws, "a removed member's new subscription is ignored").await;

    // A departed member is the same: it gets its Leave and nothing after it, here not even the Rekey
    // that follows.
    space.now = fx.now();
    let leave = space.leave(&dave);
    post_ok(remote, &mut space, &dave, &leave, "the Leave").await;
    let rekey = space.rekey(&alice, &[&alice, &carol]);
    post_ok(remote, &mut space, &alice, &rekey, "the Rekey").await;
    // Dave's socket still holds `later`, which came before its Leave, then the Leave itself.
    loop {
        let pushed = entry_of(
            &dave_ws,
            "dave gets what came before its Leave, then the Leave",
        )
        .await;
        if pushed.entry == leave.encode() {
            break;
        }
    }
    quiet(
        &dave_ws,
        "a departed member's socket gets nothing after its Leave",
    )
    .await;
    for (who, socket) in [("alice", &alice_ws), ("carol", &carol_ws)] {
        let mut got_leave = false;
        let mut got_rekey = false;
        while !(got_leave && got_rekey) {
            let pushed = entry_of(socket, &format!("{who} gets the Leave and the Rekey")).await;
            got_leave |= pushed.entry == leave.encode();
            got_rekey |= pushed.entry == rekey.encode();
        }
    }
}

/// The presence relay: see the [module documentation](self).
pub async fn check_ws_presence_relay<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob, carol, erin] = enroll_n::<R, 4>(remote, fx).await;
    let mut space = fx.space("Presence");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Viewer).await;
    invite_and_join(remote, fx, &mut space, &alice, &carol, MemberRole::Editor).await;
    let feed = space.feed;
    login(remote, &alice).await;
    let stored_before = read_feed(remote, feed).await;
    let alice_ws = subscribed(ws, &alice, &[feed], "alice subscribes").await;
    // A second socket of Alice's: another device of the same account, which is a member too.
    let alice_other = subscribed(ws, &alice, &[feed], "alice's other socket subscribes").await;
    let bob_ws = subscribed(ws, &bob, &[feed], "bob subscribes").await;
    let carol_ws = subscribed(ws, &carol, &[feed], "carol subscribes").await;
    let erin_ws = subscribed(ws, &erin, &[feed], "a stranger subscribes").await;

    // Alice's frame reaches everyone else, byte for byte, and not the socket it came from.
    let epoch = space.epoch;
    let from_alice = space.presence_frame(&alice, 0, epoch, 1, b"alice at the top");
    ok(
        alice_ws.send_presence(&from_alice).await,
        "alice sends presence",
    );
    for (who, socket) in [
        ("bob", &bob_ws),
        ("carol", &carol_ws),
        ("alice's other socket", &alice_other),
    ] {
        assert_eq!(
            presence_of(socket, &format!("{who} gets Alice's presence")).await,
            from_alice,
            "conformance: presence is relayed byte for byte"
        );
    }
    quiet(&alice_ws, "a socket does not get its own presence").await;
    quiet(&erin_ws, "a stranger gets no presence").await;

    // A Viewer sends presence too.
    let from_bob = space.presence_frame(&bob, 0, epoch, 1, b"bob, reading");
    ok(
        bob_ws.send_presence(&from_bob).await,
        "a viewer sends presence",
    );
    for (who, socket) in [
        ("alice", &alice_ws),
        ("carol", &carol_ws),
        ("alice's other socket", &alice_other),
    ] {
        assert_eq!(
            presence_of(socket, &format!("{who} gets the Viewer's presence")).await,
            from_bob
        );
    }
    quiet(&bob_ws, "a socket does not get its own presence").await;

    // A stranger's frame for the feed goes nowhere; the next frame Alice gets is Carol's.
    let from_erin = space.presence_frame(&erin, 0, epoch, 1, b"erin, outside");
    let _ = erin_ws.send_presence(&from_erin).await;
    let from_carol = space.presence_frame(&carol, 0, epoch, 1, b"carol");
    ok(
        carol_ws.send_presence(&from_carol).await,
        "carol sends presence",
    );
    assert_eq!(
        presence_of(&alice_ws, "alice gets Carol's, not the stranger's").await,
        from_carol
    );
    assert_eq!(
        peek_presence(&from_carol.envelope).map(|header| header.account_id),
        Ok(carol.account)
    );
    // Alice's other socket sends: Alice's first socket is another device of the account and gets it.
    let second = space.presence_frame(&alice, 0, epoch, 2, b"alice, again");
    ok(
        alice_other.send_presence(&second).await,
        "alice's other socket sends",
    );
    assert_eq!(
        presence_of(&alice_ws, "alice's first socket gets it").await,
        second
    );

    // Nothing of it was stored: the feed holds what it held.
    login(remote, &alice).await;
    assert_eq!(
        read_feed(remote, feed).await,
        stored_before,
        "conformance: presence is never stored"
    );
    // Nor did it hold the entries back: appends still arrive, between the frames Bob has not read.
    assert_eq!(
        presence_of(&bob_ws, "bob gets Carol's presence").await,
        from_carol
    );
    assert_eq!(
        presence_of(&bob_ws, "bob gets Alice's second frame").await,
        second
    );
    let doc = fx.doc();
    let entry = space.doc_entry(&carol, 0, doc, 1, epoch, b"after the presence");
    login(remote, &carol).await;
    let (cursor, _) = ok(remote.append(feed, &entry).await, "append");
    assert_eq!(
        entry_of(&bob_ws, "bob gets the entry between the frames")
            .await
            .cursor,
        cursor
    );
}

/// Refused presence frames are dropped and the socket stays open.
pub async fn check_ws_presence_refusals<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Refusals");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    let bob_home = FeedId::from_account(bob.account);
    let alice_ws = subscribed(ws, &alice, &[feed], "alice subscribes").await;
    let bob_ws = subscribed(ws, &bob, &[feed, bob_home], "bob subscribes").await;
    let epoch = space.epoch;

    // A header that names another account than the session's.
    let mut spoofed = space.presence_header(&bob, 0, epoch, 1);
    spoofed.account_id = alice.account;
    let _ = bob_ws
        .send_presence(&space.presence_frame_with(&spoofed, &bob, 0, b"spoofed"))
        .await;
    // A frame whose feed is not the header's.
    let mut elsewhere = space.presence_frame(&bob, 0, epoch, 2, b"elsewhere");
    elsewhere.feed = space.random_feed();
    let _ = bob_ws.send_presence(&elsewhere).await;
    // A feed Bob is not subscribed to (and not a member of), consistently named.
    let mut foreign = space.presence_header(&bob, 0, epoch, 3);
    foreign.feed = space.random_feed();
    let _ = bob_ws
        .send_presence(&space.presence_frame_with(&foreign, &bob, 0, b"foreign"))
        .await;
    // The personal feed, subscribed but not a place for presence.
    let mut home = space.presence_header(&bob, 0, epoch, 4);
    home.feed = bob_home;
    let _ = bob_ws
        .send_presence(&space.presence_frame_with(&home, &bob, 0, b"home"))
        .await;
    // Bytes that are no envelope, a frame over 4 KiB, an entry frame (only a server sends those) and an
    // unknown frame type.
    let _ = bob_ws
        .send_presence(&PresenceFrame {
            feed,
            envelope: vec![0xee; 90],
        })
        .await;
    let mut oversize = vec![2u8];
    oversize.extend_from_slice(feed.as_bytes());
    oversize.extend_from_slice(&vec![0u8; kb_types::sharing::PRESENCE_MAX_FRAME_BYTES]);
    let _ = bob_ws.send_binary(&oversize).await;
    let mut entry_frame = vec![1u8];
    entry_frame.extend_from_slice(feed.as_bytes());
    entry_frame.extend_from_slice(&[0u8; 8 + 8 + 32 + 4]);
    let _ = bob_ws.send_binary(&entry_frame).await;
    let _ = bob_ws.send_binary(&[9u8; 40]).await;
    // Text that is not a subscription changes nothing.
    let _ = bob_ws.send_text("not json").await;
    let _ = bob_ws.send_text(r#"{"type":"nonsense"}"#).await;

    // Then a frame that is fine: it is the first thing Alice gets (nothing refused reached her), and
    // the socket Bob sent them on is still open and still subscribed.
    let good = space.presence_frame(&bob, 0, epoch, 5, b"bob, properly");
    ok(bob_ws.send_presence(&good).await, "a valid frame");
    assert_eq!(
        presence_of(&alice_ws, "alice gets the one valid frame").await,
        good,
        "conformance: refused presence frames are dropped, not relayed"
    );
    let from_alice = space.presence_frame(&alice, 0, epoch, 1, b"alice");
    ok(
        alice_ws.send_presence(&from_alice).await,
        "alice sends presence",
    );
    assert_eq!(presence_of(&bob_ws, "bob still receives").await, from_alice);
    login(remote, &alice).await;
    let doc = fx.doc();
    let entry = space.doc_entry(&alice, 0, doc, 1, epoch, b"still subscribed");
    let (cursor, _) = ok(remote.append(feed, &entry).await, "append");
    assert_eq!(
        entry_of(&bob_ws, "bob's subscription survived")
            .await
            .cursor,
        cursor
    );
}

/// The excess over `PRESENCE_FRAMES_PER_SECOND` per connection is dropped.
pub async fn check_ws_presence_rate_limit<R: RemoteStore, H: WsHarness>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Rate");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    let alice_ws = subscribed(ws, &alice, &[feed], "alice subscribes").await;
    let bob_ws = subscribed(ws, &bob, &[feed], "bob subscribes").await;
    let epoch = space.epoch;
    let burst: u64 = 25;
    let frames: Vec<PresenceFrame> = (1..=burst)
        .map(|counter| space.presence_frame(&bob, 0, epoch, counter, b"moving"))
        .collect();
    for frame in &frames {
        let _ = bob_ws.send_presence(frame).await;
    }
    let mut received = Vec::new();
    while let Some(frame) = alice_ws.recv(QUIET_MS).await {
        let ServerFrame::Presence(frame) = frame else {
            panic!("conformance: only presence frames are expected");
        };
        received.push(frame);
    }
    let limit = usize::try_from(PRESENCE_FRAMES_PER_SECOND).unwrap_or(usize::MAX);
    assert!(
        received.len() >= limit && received.len() < frames.len(),
        "conformance: {} of {} frames passed; at least {limit} must and the excess must not",
        received.len(),
        frames.len()
    );
    assert_eq!(
        received,
        frames[..received.len()],
        "conformance: the frames that pass are the first ones, in order"
    );
    // A second later frames pass again: the socket was not closed.
    ws.wait_ms(1_100).await;
    let later = space.presence_frame(&bob, 0, epoch, burst + 1, b"later");
    ok(bob_ws.send_presence(&later).await, "a frame a second later");
    assert_eq!(
        presence_of(&alice_ws, "alice gets the later frame").await,
        later
    );
}

/// A device the account's log revokes loses its socket at once and cannot sign in again, while the
/// account's other device and everyone else carry on.
pub async fn check_ws_revoked_device_is_closed_and_cannot_reconnect<
    R: RemoteStore,
    H: WsHarness,
>(
    remote: &R,
    ws: &H,
    fx: &Fixture,
) {
    let [mut alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Revoked");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    // Alice's second device, published by her first.
    let second = alice.add_device(fx.now());
    login(remote, &alice).await;
    ok(
        remote
            .append_log(alice.account, &alice.encoded_log_after(0))
            .await,
        "publish the second device",
    );
    let first_ws = ok(
        ws.open_device_socket(&alice, 0).await,
        "open the first device's socket",
    );
    ok(first_ws.subscribe(&[feed]).await, "subscribe");
    let second_ws = ok(
        ws.open_device_socket(&alice, second).await,
        "open the second device's socket",
    );
    ok(second_ws.subscribe(&[feed]).await, "subscribe");
    let bob_ws = subscribed(ws, &bob, &[feed], "bob subscribes").await;
    let epoch = space.epoch;
    let before = space.presence_frame(&bob, 0, epoch, 1, b"before");
    ok(bob_ws.send_presence(&before).await, "bob sends presence");
    assert_eq!(
        presence_of(&first_ws, "the first device gets it").await,
        before
    );
    assert_eq!(
        presence_of(&second_ws, "the second device gets it").await,
        before
    );

    // The second device revokes the first.
    alice.revoke_device(0, fx.now() + 1);
    ok(
        remote
            .authenticate(
                alice.account,
                alice.device_id(second),
                &alice.signer(second),
            )
            .await,
        "the second device signs in",
    );
    ok(
        remote
            .append_log(alice.account, &alice.encoded_log_after(1))
            .await,
        "publish the revocation",
    );
    assert!(
        first_ws.is_closed(WAIT_MS).await,
        "conformance: a revoked device's socket is closed"
    );
    // It cannot come back: it cannot sign in, so it cannot open a socket.
    assert!(
        ws.open_device_socket(&alice, 0).await.is_err(),
        "conformance: a revoked device cannot reconnect"
    );
    // The others are not affected, and the revoked device is out of the relay.
    let after = space.presence_frame(&bob, 0, epoch, 2, b"after");
    ok(bob_ws.send_presence(&after).await, "bob sends presence");
    assert_eq!(
        presence_of(&second_ws, "the second device still gets presence").await,
        after
    );
    let from_second = space.presence_frame(&alice, second, epoch, 1, b"second");
    ok(
        second_ws.send_presence(&from_second).await,
        "the second device sends presence",
    );
    assert_eq!(
        presence_of(&bob_ws, "bob gets the second device's presence").await,
        from_second
    );
    assert!(!second_ws.is_closed(QUIET_MS).await);
}
