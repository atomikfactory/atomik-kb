//! WebSocket v2 and presence (M13 slice 3, ADR-0050, `docs/security/sharing.md` section 9,
//! `docs/development/milestone-13.md` section 3.1): the frames of `GET /v2/ws`, the presence envelope
//! (header, body, seal and open) and the types the core, the driver and the servers share.
//!
//! The codecs are strict and total: truncation, trailing bytes, unknown frame types, the size limits
//! (in both directions) and unparseable text are errors, never a panic whatever the bytes. The
//! reference remote's socket ([`MemoryRemote::ws_open`](super::MemoryRemote)) and `kb-server` apply
//! the same rules to the same frames; the frame rate limit they share is [`FrameLimiter`].
//!
//! # Frames (integers little-endian, ids 16 raw bytes)
//!
//! ```text
//! client -> server, text    {"type":"subscribe","feeds":["<feed uuid>", ...]}
//! server -> client, text    {"type":"changed","feed":"<feed uuid>","cursor":N}
//! server -> client, binary  u8 1 | feed | u64 prev_cursor | u64 cursor | [32] chain_after | entry   (65 + len)
//! both ways, binary         u8 2 | feed | envelope of kind Presence                                  (17 + len)
//! ```
//!
//! - **`subscribe`** *replaces* the connection's subscription set (an empty list unsubscribes from
//!   everything); the server checks each feed against the session's access list and ignores the ones
//!   it may not read, so a stranger learns nothing (a subscription to an unknown feed and to a
//!   forbidden one look alike, and nothing is answered). At most `MAX_FEEDS_PER_ACCOUNT` feeds; a
//!   text frame is at most [`CLIENT_MESSAGE_MAX_BYTES`]. A removed or departed account's
//!   subscription ends at its `Remove` or `Leave`: it receives that entry (it may read the feed up
//!   to it) and nothing after. There is no acknowledgement: a client that must not miss anything
//!   pulls after it subscribed (the driver does, and every 60 s).
//! - **`changed`**: the feed's newest cursor is `cursor`; sent for every item the server did not
//!   relay inline (an item over `INLINE_ENTRY_MAX_BYTES`), and for every subscribed feed of a
//!   connection whose receiver lagged (the server could not keep up). A client whose cursor for the
//!   feed is lower pulls it.
//! - **entry frame**: one feed item (document entry, keyring entry or membership entry; the item's
//!   bytes exactly as `changes` returns them) with its position: the cursor of the item before it in
//!   the feed (`prev_cursor`, 0 for the first), its own cursor and the chain hash after it. Sent to
//!   **every** subscribed socket of a current member, the appending device's own socket included
//!   (that device's cursor must stay contiguous across its own entries: the core counts it as
//!   `skipped_own`). A receiver applies it when `prev_cursor` equals its cursor for the feed and
//!   pulls otherwise; there is no path that applies content without storing it.
//! - **presence frame**: the sender's sealed envelope behind the feed it is for. The server relays it
//!   to the other subscribed sockets of the feed's current members (never to the sender's own socket,
//!   but to the other devices of the sender's account), stores nothing, and refuses a frame whose
//!   header names another account than the session's or another feed than the frame's, one over
//!   `PRESENCE_MAX_FRAME_BYTES`, one for a feed the sending socket is not subscribed to (a
//!   subscription is a checked membership, so only current members send; presence is for shared
//!   feeds, never the personal one), a frame that does not parse, and the excess over
//!   `PRESENCE_FRAMES_PER_SECOND` per connection ([`FrameLimiter`]). A refused frame is dropped
//!   and the socket stays open. A removed member's socket neither sends nor receives presence
//!   after its `Remove`.
//!
//! # The presence envelope
//!
//! ```text
//! header   u8 2 | feed | account | device | u32 epoch | u64 counter                  (61)
//! body     u8 1 | doc | u8 state | blob update            (doc: the page's document id, all zeros for none;
//!                                                          state: PresenceState::to_byte; blob: u32 len + bytes)
//! envelope kind Presence, the body bucket-padded and sealed under `kb_keys::space_keys::presence_key(SK[epoch], feed)`,
//!          signed by the sending device (context `envelope/presence`)
//! ```
//!
//! `epoch` is the space key epoch the frame is sealed under, `counter` grows by one per frame of a
//! device (receivers drop a frame whose counter is not above the last they accepted from that
//! device). The colour index inside `update` is upstream's format and is not trusted.

use std::collections::VecDeque;

use kb_crypto::{EnvelopeKind, Secret32, SigningKey, VerifyingKey};
use kb_types::sharing::{
    INLINE_ENTRY_MAX_BYTES, MAX_FEEDS_PER_ACCOUNT, PRESENCE_FRAMES_PER_SECOND,
    PRESENCE_MAX_FRAME_BYTES,
};
use kb_types::sync::PROTOCOL_VERSION;
use kb_types::{AccountId, DeviceId, DocId, FeedId, PresenceState, Rng};
use serde::Deserialize;
use zeroize::Zeroizing;

use super::{read_feed, read_version};
use crate::codec::{Reader, put_blob};
use crate::{
    SyncError, decode_envelope, open_body, read_account, read_device, read_doc, seal_body,
};

/// First byte of a binary frame that carries an entry.
pub const FRAME_ENTRY: u8 = 1;
/// First byte of a binary frame that carries a presence envelope.
pub const FRAME_PRESENCE: u8 = 2;
/// Bytes of an entry frame before the entry: `1 | feed | prev_cursor | cursor | chain_after`.
pub const ENTRY_FRAME_PREFIX_LEN: usize = 1 + 16 + 8 + 8 + 32;
/// Bytes of a presence frame before the envelope: `2 | feed`.
pub const PRESENCE_FRAME_PREFIX_LEN: usize = 1 + 16;
/// Bytes of a presence envelope's header.
pub const PRESENCE_HEADER_LEN: usize = 1 + 16 + 16 + 16 + 4 + 8;
/// The version byte of the presence body.
pub const PRESENCE_BODY_VERSION: u8 = 1;
/// The largest message (text or binary) a server accepts from a client: a `subscribe` for 100 feeds
/// is about 4 KiB and a presence frame at most [`PRESENCE_MAX_FRAME_BYTES`]. A transport layer
/// closes a connection that sends more than this (a message beyond it is not a frame to ignore).
pub const CLIENT_MESSAGE_MAX_BYTES: usize = 16 * 1024;

// ----- frames --------------------------------------------------------------------------------

/// One feed item relayed inline (the binary frame `1`): see the [module documentation](self).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineEntry {
    /// The feed.
    pub feed: FeedId,
    /// The cursor of the item before this one in the feed, 0 when it is the first.
    pub prev_cursor: u64,
    /// This item's cursor.
    pub cursor: u64,
    /// The feed's chain hash after this item (`chain_next` over the bytes of every item up to it).
    pub chain_after: [u8; 32],
    /// The item's bytes, as `changes` returns them (at most `INLINE_ENTRY_MAX_BYTES`).
    pub entry: Vec<u8>,
}

/// A presence frame (the binary frame `2`): the sender's sealed envelope for a feed. The server and
/// the clients treat the envelope as bytes; [`peek_presence`] reads its header and [`open_presence`]
/// opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceFrame {
    /// The feed the presence is in (the envelope's header names the same one).
    pub feed: FeedId,
    /// `Envelope::encode` bytes of kind `Presence` (see [`seal_presence`]).
    pub envelope: Vec<u8>,
}

/// What the server sends a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFrame {
    /// The text frame `changed`: the feed's newest cursor.
    Changed {
        /// The feed.
        feed: FeedId,
        /// Its newest cursor.
        cursor: u64,
    },
    /// The binary frame `1`.
    Entry(InlineEntry),
    /// The binary frame `2`, relayed from another member's device.
    Presence(PresenceFrame),
}

/// What a client sends the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    /// The text frame `subscribe`: the feeds the connection wants (replaces the previous set).
    Subscribe(Vec<FeedId>),
    /// The binary frame `2`.
    Presence(PresenceFrame),
}

/// The wire form of a feed id in the JSON frames: the UUID text.
fn feed_text(feed: FeedId) -> String {
    feed.to_string()
}

/// `{"type":"subscribe","feeds":[...]}`. More feeds than `MAX_FEEDS_PER_ACCOUNT` are
/// [`SyncError::Protocol`].
pub fn encode_subscribe(feeds: &[FeedId]) -> Result<String, SyncError> {
    if feeds.len() > MAX_FEEDS_PER_ACCOUNT {
        return Err(SyncError::Protocol(format!(
            "a subscription names at most {MAX_FEEDS_PER_ACCOUNT} feeds"
        )));
    }
    let mut out = String::with_capacity(32 + feeds.len() * 39);
    out.push_str(r#"{"type":"subscribe","feeds":["#);
    for (index, feed) in feeds.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&feed_text(*feed));
        out.push('"');
    }
    out.push_str("]}");
    Ok(out)
}

/// `{"type":"changed","feed":"<uuid>","cursor":N}`.
pub fn encode_changed(feed: FeedId, cursor: u64) -> Result<String, SyncError> {
    Ok(format!(
        r#"{{"type":"changed","feed":"{}","cursor":{cursor}}}"#,
        feed_text(feed)
    ))
}

/// The binary frame `1`. An entry over `INLINE_ENTRY_MAX_BYTES` is [`SyncError::Protocol`] (the
/// server sends `changed` instead).
pub fn encode_entry_frame(inline: &InlineEntry) -> Result<Vec<u8>, SyncError> {
    if inline.entry.len() > INLINE_ENTRY_MAX_BYTES {
        return Err(SyncError::Protocol(format!(
            "an inline entry is at most {INLINE_ENTRY_MAX_BYTES} bytes"
        )));
    }
    let mut out = Vec::with_capacity(ENTRY_FRAME_PREFIX_LEN + inline.entry.len());
    out.push(FRAME_ENTRY);
    out.extend_from_slice(inline.feed.as_bytes());
    out.extend_from_slice(&inline.prev_cursor.to_le_bytes());
    out.extend_from_slice(&inline.cursor.to_le_bytes());
    out.extend_from_slice(&inline.chain_after);
    out.extend_from_slice(&inline.entry);
    Ok(out)
}

/// The binary frame `2`. A whole frame over `PRESENCE_MAX_FRAME_BYTES` is
/// [`SyncError::Protocol`].
pub fn encode_presence_frame(frame: &PresenceFrame) -> Result<Vec<u8>, SyncError> {
    let len = PRESENCE_FRAME_PREFIX_LEN.saturating_add(frame.envelope.len());
    if len > PRESENCE_MAX_FRAME_BYTES {
        return Err(SyncError::Protocol(format!(
            "a presence frame is at most {PRESENCE_MAX_FRAME_BYTES} bytes"
        )));
    }
    let mut out = Vec::with_capacity(len);
    out.push(FRAME_PRESENCE);
    out.extend_from_slice(frame.feed.as_bytes());
    out.extend_from_slice(&frame.envelope);
    Ok(out)
}

/// The shape of the text frames a server sends (anything else is [`SyncError::Encoding`]).
#[derive(Deserialize)]
#[serde(tag = "type")]
enum ServerText {
    #[serde(rename = "changed")]
    Changed { feed: FeedId, cursor: u64 },
}

/// The shape of the text frames a client sends.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientText {
    #[serde(rename = "subscribe")]
    Subscribe { feeds: Vec<FeedId> },
}

/// Reads a text frame a server sent (`changed`); anything else, unknown `type`s included, is
/// [`SyncError::Encoding`] (the client ignores what it does not understand).
pub fn decode_server_text(text: &str) -> Result<ServerFrame, SyncError> {
    if text.len() > CLIENT_MESSAGE_MAX_BYTES {
        return Err(SyncError::Encoding("a text frame is too long".to_owned()));
    }
    match serde_json::from_str::<ServerText>(text) {
        Ok(ServerText::Changed { feed, cursor }) => Ok(ServerFrame::Changed { feed, cursor }),
        Err(_) => Err(SyncError::Encoding(
            "the text frame is not a changed frame".to_owned(),
        )),
    }
}

/// Reads an entry frame after its type byte: the feed, both cursors, the chain hash and, to the end
/// of the frame, the entry.
fn read_entry_frame(bytes: &[u8]) -> Result<InlineEntry, SyncError> {
    let mut r = Reader::new(bytes, "entry frame");
    let feed = read_feed(&mut r)?;
    let prev_cursor = r.u64("previous cursor")?;
    let cursor = r.u64("cursor")?;
    let chain_after = r.array::<32>("chain hash")?;
    let entry = r.take(r.remaining(), "entry")?.to_vec();
    r.finish()?;
    if entry.len() > INLINE_ENTRY_MAX_BYTES {
        return Err(SyncError::Encoding(format!(
            "entry frame: an entry of {} bytes is over the inline limit",
            entry.len()
        )));
    }
    Ok(InlineEntry {
        feed,
        prev_cursor,
        cursor,
        chain_after,
        entry,
    })
}

/// Reads a presence frame after its type byte: the feed and, to the end of the frame, the envelope.
/// `total` is the whole frame's length, type byte included.
fn read_presence_frame(bytes: &[u8], total: usize) -> Result<PresenceFrame, SyncError> {
    if total > PRESENCE_MAX_FRAME_BYTES {
        return Err(SyncError::Encoding(format!(
            "presence frame: {total} bytes is over the limit"
        )));
    }
    let mut r = Reader::new(bytes, "presence frame");
    let feed = read_feed(&mut r)?;
    let envelope = r.take(r.remaining(), "envelope")?.to_vec();
    r.finish()?;
    Ok(PresenceFrame { feed, envelope })
}

/// Reads a binary frame a server sent (`1` or `2`). Truncation, trailing bytes, an unknown first
/// byte, an entry over `INLINE_ENTRY_MAX_BYTES` and a presence frame over
/// `PRESENCE_MAX_FRAME_BYTES` are [`SyncError::Encoding`] (never a panic, whatever the bytes).
pub fn decode_server_binary(bytes: &[u8]) -> Result<ServerFrame, SyncError> {
    let Some((kind, rest)) = bytes.split_first() else {
        return Err(SyncError::Encoding(
            "websocket frame: truncated frame type".to_owned(),
        ));
    };
    match *kind {
        FRAME_ENTRY => read_entry_frame(rest).map(ServerFrame::Entry),
        FRAME_PRESENCE => read_presence_frame(rest, bytes.len()).map(ServerFrame::Presence),
        other => Err(SyncError::Encoding(format!(
            "websocket frame: unknown frame type {other}"
        ))),
    }
}

/// Reads a text frame a client sent (`subscribe`); anything else is [`SyncError::Encoding`], more
/// than `MAX_FEEDS_PER_ACCOUNT` feeds [`SyncError::Protocol`].
pub fn decode_client_text(text: &str) -> Result<ClientFrame, SyncError> {
    if text.len() > CLIENT_MESSAGE_MAX_BYTES {
        return Err(SyncError::Encoding("a text frame is too long".to_owned()));
    }
    match serde_json::from_str::<ClientText>(text) {
        Ok(ClientText::Subscribe { feeds }) => {
            if feeds.len() > MAX_FEEDS_PER_ACCOUNT {
                return Err(SyncError::Protocol(format!(
                    "a subscription names at most {MAX_FEEDS_PER_ACCOUNT} feeds"
                )));
            }
            Ok(ClientFrame::Subscribe(feeds))
        }
        Err(_) => Err(SyncError::Encoding(
            "the text frame is not a subscribe frame".to_owned(),
        )),
    }
}

/// Reads a binary frame a client sent: only `2` is allowed (an entry frame from a client is
/// [`SyncError::Protocol`]); the rest as [`decode_server_binary`].
pub fn decode_client_binary(bytes: &[u8]) -> Result<ClientFrame, SyncError> {
    let Some((kind, rest)) = bytes.split_first() else {
        return Err(SyncError::Encoding(
            "websocket frame: truncated frame type".to_owned(),
        ));
    };
    match *kind {
        FRAME_PRESENCE => read_presence_frame(rest, bytes.len()).map(ClientFrame::Presence),
        FRAME_ENTRY => Err(SyncError::Protocol(
            "a client does not send entry frames".to_owned(),
        )),
        other => Err(SyncError::Encoding(format!(
            "websocket frame: unknown frame type {other}"
        ))),
    }
}

// ----- the frame limiter ----------------------------------------------------------------------

/// A sliding-window counter: at most `limit` frames in any `window_ms`. `MemorySocket` and `kb-server`
/// share it, so the rate of the presence frames (`PRESENCE_FRAMES_PER_SECOND` per connection) is one
/// rule in two places. Time is the caller's clock, Unix or monotonic ms; a clock that goes back is
/// tolerated (the frames still in the window keep counting).
#[derive(Debug, Clone)]
pub struct FrameLimiter {
    limit: usize,
    window_ms: i64,
    recent: VecDeque<i64>,
}

impl FrameLimiter {
    /// At most `limit` frames per `window_ms` milliseconds (a `limit` of 0 lets nothing through).
    #[must_use]
    pub fn new(limit: u32, window_ms: i64) -> Self {
        Self {
            limit: usize::try_from(limit).unwrap_or(usize::MAX),
            window_ms,
            recent: VecDeque::new(),
        }
    }

    /// The presence rate: `PRESENCE_FRAMES_PER_SECOND` frames per second.
    #[must_use]
    pub fn presence() -> Self {
        Self::new(PRESENCE_FRAMES_PER_SECOND, 1_000)
    }

    /// Counts a frame that arrives at `now_ms`; `false` when it exceeds the rate (the caller drops
    /// it; a dropped frame does not count against later ones).
    pub fn allow(&mut self, now_ms: i64) -> bool {
        let oldest_in_window = now_ms.saturating_sub(self.window_ms);
        while self
            .recent
            .front()
            .is_some_and(|at| *at <= oldest_in_window)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= self.limit {
            return false;
        }
        self.recent.push_back(now_ms);
        true
    }
}

// ----- the presence envelope -----------------------------------------------------------------

/// Plaintext header of a presence envelope (what the server sees and checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceHeader {
    /// The feed the frame is for.
    pub feed: FeedId,
    /// The sender's account (the server refuses a frame whose account is not the session's).
    pub account_id: AccountId,
    /// The sender's device, which signed the envelope.
    pub device_id: DeviceId,
    /// The space key epoch the body is sealed under.
    pub epoch: u32,
    /// Grows by one per frame of a device (a receiver accepts only a larger counter than the last
    /// it accepted from that device in that feed).
    pub counter: u64,
}

/// The sealed content of a presence frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceBody {
    /// The document the device is in (a page's document id is its page id's bytes); all zeros
    /// (`KEYRING_DOC_ID`) for none.
    pub doc_id: DocId,
    /// What the device is doing.
    pub state: PresenceState,
    /// The sender's `EphemeralStore` update (at most `PRESENCE_UPDATE_MAX_BYTES`; opaque here).
    pub update: Vec<u8>,
}

/// The header bytes of a presence envelope ([`PRESENCE_HEADER_LEN`]); the envelope header of
/// [`seal_presence`].
#[must_use]
pub fn encode_presence_header(header: &PresenceHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(PRESENCE_HEADER_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(header.feed.as_bytes());
    out.extend_from_slice(header.account_id.as_bytes());
    out.extend_from_slice(header.device_id.as_bytes());
    out.extend_from_slice(&header.epoch.to_le_bytes());
    out.extend_from_slice(&header.counter.to_le_bytes());
    out
}

fn decode_presence_header(bytes: &[u8]) -> Result<PresenceHeader, SyncError> {
    let mut r = Reader::new(bytes, "presence header");
    read_version(&mut r)?;
    let header = PresenceHeader {
        feed: read_feed(&mut r)?,
        account_id: read_account(&mut r)?,
        device_id: read_device(&mut r)?,
        epoch: r.u32("key epoch")?,
        counter: r.u64("counter")?,
    };
    r.finish()?;
    Ok(header)
}

/// Reads the header of a presence envelope without a key or a signature check: what a server routes
/// and checks on (the account against the session's, the feed against the frame's). Malformed bytes,
/// another envelope kind and a v1 header are errors.
pub fn peek_presence(bytes: &[u8]) -> Result<PresenceHeader, SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Presence)?;
    decode_presence_header(&envelope.header)
}

/// Seals a presence body under the presence key of its epoch (`kb_keys::space_keys::presence_key`),
/// signed by the sending device: the header in the clear, the body bucket-padded and encrypted. A
/// body that cannot be represented gives an empty result, which every decoder refuses, like
/// [`seal_entry`](super::seal_entry).
#[must_use]
pub fn seal_presence(
    header: &PresenceHeader,
    body: &PresenceBody,
    presence_key: &Secret32,
    signer: &SigningKey,
    rng: &dyn Rng,
) -> Vec<u8> {
    seal_body(
        EnvelopeKind::Presence,
        &encode_presence_header(header),
        encode_presence_body(body),
        presence_key,
        signer,
        rng,
    )
}

/// `u8 1 | doc | u8 state | blob update`; `None` when the update does not fit a `u32` length.
fn encode_presence_body(body: &PresenceBody) -> Option<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::with_capacity(1 + 16 + 1 + 4 + body.update.len()));
    out.push(PRESENCE_BODY_VERSION);
    out.extend_from_slice(body.doc_id.as_bytes());
    out.push(body.state.to_byte());
    put_blob(&mut out, &body.update)?;
    Some(out)
}

fn decode_presence_body(bytes: &[u8]) -> Result<PresenceBody, SyncError> {
    let mut r = Reader::new(bytes, "presence body");
    let version = r.u8("body version")?;
    if version != PRESENCE_BODY_VERSION {
        return Err(SyncError::Encoding(format!(
            "presence body: unsupported version {version}"
        )));
    }
    let doc_id = read_doc(&mut r)?;
    let state_byte = r.u8("state")?;
    let state = PresenceState::from_byte(state_byte)
        .ok_or_else(|| SyncError::Encoding(format!("presence body: unknown state {state_byte}")))?;
    let update = r.blob("update")?.to_vec();
    r.finish()?;
    Ok(PresenceBody {
        doc_id,
        state,
        update,
    })
}

/// Verifies the signature against the sending device's key, then decrypts a presence envelope; the
/// signature is checked before anything is decrypted ([`SyncError::Authentication`] for a bad one, a
/// wrong key or a tampered header). The caller (the core) has already chosen the key by the header's
/// epoch and the verifier by the header's account and device.
pub fn open_presence(
    bytes: &[u8],
    presence_key: &Secret32,
    verifier: &VerifyingKey,
) -> Result<(PresenceHeader, PresenceBody), SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Presence)?;
    let header = decode_presence_header(&envelope.header)?;
    let content = open_body(bytes, &envelope, presence_key, verifier)?;
    Ok((header, decode_presence_body(&content)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn header() -> PresenceHeader {
        PresenceHeader {
            feed: FeedId::from_bytes([1; 16]),
            account_id: AccountId::from_bytes([2; 16]),
            device_id: DeviceId::from_bytes([3; 16]),
            epoch: 0x0102_0304,
            counter: 0x1112_1314_1516_1718,
        }
    }

    #[test]
    fn the_header_layout_is_the_documented_one() {
        let bytes = encode_presence_header(&header());
        assert_eq!(bytes.len(), PRESENCE_HEADER_LEN);
        assert_eq!(bytes.len(), 61);
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[1..17], &[1; 16]);
        assert_eq!(&bytes[17..33], &[2; 16]);
        assert_eq!(&bytes[33..49], &[3; 16]);
        assert_eq!(&bytes[49..53], &[4, 3, 2, 1]);
        assert_eq!(
            &bytes[53..61],
            &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]
        );
        assert_eq!(decode_presence_header(&bytes).unwrap(), header());
    }

    #[test]
    fn a_malformed_header_is_refused() {
        let bytes = encode_presence_header(&header());
        // Truncated, with a trailing byte, and with the version byte of v1 or an unknown one.
        assert!(decode_presence_header(&bytes[..60]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_presence_header(&longer).is_err());
        let mut v1 = bytes.clone();
        v1[0] = 1;
        assert!(matches!(
            decode_presence_header(&v1),
            Err(SyncError::Protocol(_))
        ));
        let mut unknown = bytes;
        unknown[0] = 9;
        assert!(matches!(
            decode_presence_header(&unknown),
            Err(SyncError::Encoding(_))
        ));
        assert!(decode_presence_header(&[]).is_err());
    }

    #[test]
    fn the_frame_constants_add_up() {
        assert_eq!(ENTRY_FRAME_PREFIX_LEN, 65);
        assert_eq!(PRESENCE_FRAME_PREFIX_LEN, 17);
        assert_eq!((FRAME_ENTRY, FRAME_PRESENCE), (1, 2));
        // The prefixes leave room for an entry and for an envelope under the documented limits.
        const { assert!(ENTRY_FRAME_PREFIX_LEN < kb_types::sharing::INLINE_ENTRY_MAX_BYTES) };
        const {
            assert!(
                PRESENCE_FRAME_PREFIX_LEN + PRESENCE_HEADER_LEN
                    < kb_types::sharing::PRESENCE_MAX_FRAME_BYTES
            );
        };
    }

    // ----- the codecs ------------------------------------------------------------------------

    fn feed(n: u8) -> FeedId {
        FeedId::from_bytes([n; 16])
    }

    fn inline(entry_len: usize) -> InlineEntry {
        InlineEntry {
            feed: feed(7),
            prev_cursor: 0x0102_0304_0506_0708,
            cursor: 0x1112_1314_1516_1718,
            chain_after: [0xcd; 32],
            entry: (0..entry_len).map(|i| (i % 251) as u8).collect(),
        }
    }

    #[test]
    fn subscribe_is_compact_json_in_the_documented_order() {
        let text = encode_subscribe(&[feed(1), feed(2)]).unwrap();
        assert_eq!(
            text,
            r#"{"type":"subscribe","feeds":["01010101-0101-0101-0101-010101010101","02020202-0202-0202-0202-020202020202"]}"#
        );
        assert_eq!(
            encode_subscribe(&[]).unwrap(),
            r#"{"type":"subscribe","feeds":[]}"#
        );
        assert_eq!(
            decode_client_text(&text).unwrap(),
            ClientFrame::Subscribe(vec![feed(1), feed(2)])
        );
        assert_eq!(
            decode_client_text(r#"{"type":"subscribe","feeds":[]}"#).unwrap(),
            ClientFrame::Subscribe(Vec::new())
        );
    }

    #[test]
    fn subscribe_carries_at_most_the_feed_limit() {
        let ids: Vec<FeedId> = (0..=MAX_FEEDS_PER_ACCOUNT)
            .map(|n| FeedId::from_bytes([(n + 1) as u8; 16]))
            .collect();
        let max = &ids[..MAX_FEEDS_PER_ACCOUNT];
        let text = encode_subscribe(max).unwrap();
        assert!(text.len() < CLIENT_MESSAGE_MAX_BYTES);
        assert_eq!(
            decode_client_text(&text).unwrap(),
            ClientFrame::Subscribe(max.to_vec())
        );
        assert!(matches!(
            encode_subscribe(&ids),
            Err(SyncError::Protocol(_))
        ));
        // A frame written by hand with one feed too many is refused by the decoder, too.
        let mut over = String::from(r#"{"type":"subscribe","feeds":["#);
        for (i, id) in ids.iter().enumerate() {
            if i > 0 {
                over.push(',');
            }
            over.push_str(&format!("\"{id}\""));
        }
        over.push_str("]}");
        assert!(matches!(
            decode_client_text(&over),
            Err(SyncError::Protocol(_))
        ));
    }

    #[test]
    fn a_client_text_frame_must_be_a_subscribe() {
        for text in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"type":"subscribe"}"#,
            r#"{"type":"subscribe","feeds":null}"#,
            r#"{"type":"subscribe","feeds":["not-a-uuid"]}"#,
            r#"{"type":"subscribe","feeds":[7]}"#,
            r#"{"type":"changed","feed":"01010101-0101-0101-0101-010101010101","cursor":1}"#,
            r#"{"type":"unsubscribe","feeds":[]}"#,
            r#"{"type":7,"feeds":[]}"#,
        ] {
            assert!(
                matches!(decode_client_text(text), Err(SyncError::Encoding(_))),
                "{text}"
            );
        }
        // Fields a later version may add are ignored.
        assert_eq!(
            decode_client_text(r#"{"type":"subscribe","feeds":[],"since":3}"#).unwrap(),
            ClientFrame::Subscribe(Vec::new())
        );
        // A text frame over the client limit is refused before it is parsed.
        let padded = format!(
            r#"{{"type":"subscribe","feeds":[],"pad":"{}"}}"#,
            "x".repeat(CLIENT_MESSAGE_MAX_BYTES)
        );
        assert!(matches!(
            decode_client_text(&padded),
            Err(SyncError::Encoding(_))
        ));
    }

    #[test]
    fn changed_is_compact_json_in_the_documented_order() {
        let text = encode_changed(feed(3), 42).unwrap();
        assert_eq!(
            text,
            r#"{"type":"changed","feed":"03030303-0303-0303-0303-030303030303","cursor":42}"#
        );
        assert_eq!(
            decode_server_text(&text).unwrap(),
            ServerFrame::Changed {
                feed: feed(3),
                cursor: 42
            }
        );
        let max = encode_changed(feed(3), u64::MAX).unwrap();
        assert_eq!(
            decode_server_text(&max).unwrap(),
            ServerFrame::Changed {
                feed: feed(3),
                cursor: u64::MAX
            }
        );
    }

    #[test]
    fn a_server_text_frame_must_be_changed_and_unknown_ones_are_ignorable() {
        for text in [
            "",
            "{}",
            "not json",
            r#"{"type":"changed"}"#,
            r#"{"type":"changed","cursor":1}"#,
            r#"{"type":"changed","feed":"01010101-0101-0101-0101-010101010101"}"#,
            r#"{"type":"changed","feed":"01010101-0101-0101-0101-010101010101","cursor":-1}"#,
            r#"{"type":"changed","feed":"01010101-0101-0101-0101-010101010101","cursor":1.5}"#,
            r#"{"type":"changed","feed":"01010101-0101-0101-0101-010101010101","cursor":18446744073709551616}"#,
            // v1's feed-less ping is not a v2 frame.
            r#"{"type":"changed","cursor":9}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"subscribe","feeds":[]}"#,
        ] {
            assert!(
                matches!(decode_server_text(text), Err(SyncError::Encoding(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn the_entry_frame_layout_is_the_documented_one() {
        let frame = inline(5);
        let bytes = encode_entry_frame(&frame).unwrap();
        assert_eq!(bytes.len(), ENTRY_FRAME_PREFIX_LEN + 5);
        assert_eq!(bytes[0], 1);
        assert_eq!(&bytes[1..17], &[7; 16]);
        assert_eq!(&bytes[17..25], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(&bytes[25..33], &0x1112_1314_1516_1718u64.to_le_bytes());
        assert_eq!(&bytes[33..65], &[0xcd; 32]);
        assert_eq!(&bytes[65..], &frame.entry[..]);
        assert_eq!(
            decode_server_binary(&bytes).unwrap(),
            ServerFrame::Entry(frame)
        );
    }

    #[test]
    fn the_entry_frame_takes_an_entry_up_to_the_inline_limit_and_not_a_byte_more() {
        let biggest = inline(INLINE_ENTRY_MAX_BYTES);
        let bytes = encode_entry_frame(&biggest).unwrap();
        assert_eq!(bytes.len(), ENTRY_FRAME_PREFIX_LEN + INLINE_ENTRY_MAX_BYTES);
        assert_eq!(
            decode_server_binary(&bytes).unwrap(),
            ServerFrame::Entry(biggest)
        );
        assert!(matches!(
            encode_entry_frame(&inline(INLINE_ENTRY_MAX_BYTES + 1)),
            Err(SyncError::Protocol(_))
        ));
        let mut over = bytes;
        over.push(0);
        assert!(matches!(
            decode_server_binary(&over),
            Err(SyncError::Encoding(_))
        ));
    }

    #[test]
    fn the_entry_frame_is_refused_when_truncated_and_is_never_a_client_frame() {
        let bytes = encode_entry_frame(&inline(10)).unwrap();
        for len in 0..ENTRY_FRAME_PREFIX_LEN {
            assert!(
                matches!(
                    decode_server_binary(&bytes[..len]),
                    Err(SyncError::Encoding(_))
                ),
                "{len}"
            );
        }
        assert!(matches!(
            decode_client_binary(&bytes),
            Err(SyncError::Protocol(_))
        ));
    }

    #[test]
    fn the_presence_frame_layout_is_the_documented_one() {
        let envelope: Vec<u8> = (0..200).map(|i| i as u8).collect();
        let frame = PresenceFrame {
            feed: feed(9),
            envelope: envelope.clone(),
        };
        let bytes = encode_presence_frame(&frame).unwrap();
        assert_eq!(bytes.len(), PRESENCE_FRAME_PREFIX_LEN + 200);
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[1..17], &[9; 16]);
        assert_eq!(&bytes[17..], &envelope[..]);
        assert_eq!(
            decode_server_binary(&bytes).unwrap(),
            ServerFrame::Presence(frame.clone())
        );
        assert_eq!(
            decode_client_binary(&bytes).unwrap(),
            ClientFrame::Presence(frame)
        );
    }

    #[test]
    fn the_presence_frame_is_at_most_4_kib_as_a_whole() {
        let fits = PresenceFrame {
            feed: feed(9),
            envelope: vec![5; PRESENCE_MAX_FRAME_BYTES - PRESENCE_FRAME_PREFIX_LEN],
        };
        let bytes = encode_presence_frame(&fits).unwrap();
        assert_eq!(bytes.len(), PRESENCE_MAX_FRAME_BYTES);
        assert!(decode_server_binary(&bytes).is_ok());
        assert!(decode_client_binary(&bytes).is_ok());
        let too_big = PresenceFrame {
            feed: feed(9),
            envelope: vec![5; PRESENCE_MAX_FRAME_BYTES - PRESENCE_FRAME_PREFIX_LEN + 1],
        };
        assert!(matches!(
            encode_presence_frame(&too_big),
            Err(SyncError::Protocol(_))
        ));
        let mut over = bytes;
        over.push(0);
        assert!(matches!(
            decode_server_binary(&over),
            Err(SyncError::Encoding(_))
        ));
        assert!(matches!(
            decode_client_binary(&over),
            Err(SyncError::Encoding(_))
        ));
    }

    #[test]
    fn a_binary_frame_with_an_unknown_type_or_too_short_is_refused() {
        for bytes in [
            &[][..],
            &[0],
            &[3, 0, 0, 0],
            &[255; 80],
            &[2],
            &[2, 1, 2, 3],
            &[2; 16],
        ] {
            assert!(
                matches!(decode_server_binary(bytes), Err(SyncError::Encoding(_))),
                "{bytes:?}"
            );
            assert!(
                matches!(decode_client_binary(bytes), Err(SyncError::Encoding(_))),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn decoders_never_panic_on_any_prefix_or_flipped_byte_of_a_frame() {
        let entry = encode_entry_frame(&inline(40)).unwrap();
        let presence = encode_presence_frame(&PresenceFrame {
            feed: feed(1),
            envelope: vec![0xab; 90],
        })
        .unwrap();
        let text = encode_subscribe(&[feed(1), feed(2)]).unwrap();
        for bytes in [&entry, &presence] {
            for len in 0..=bytes.len() {
                let _ = decode_server_binary(&bytes[..len]);
                let _ = decode_client_binary(&bytes[..len]);
            }
            for i in 0..bytes.len() {
                let mut copy = bytes.clone();
                copy[i] ^= 0xff;
                let _ = decode_server_binary(&copy);
                let _ = decode_client_binary(&copy);
            }
        }
        for len in 0..=text.len() {
            if text.is_char_boundary(len) {
                let _ = decode_client_text(&text[..len]);
                let _ = decode_server_text(&text[..len]);
            }
        }
    }

    // ----- the presence envelope -------------------------------------------------------------

    fn presence_key() -> Secret32 {
        Secret32::from_bytes([0x61; 32])
    }

    fn device_key() -> SigningKey {
        SigningKey::from_seed(&Secret32::from_bytes([0x62; 32]))
    }

    fn body(update_len: usize) -> PresenceBody {
        PresenceBody {
            doc_id: DocId::from_bytes([8; 16]),
            state: PresenceState::Editing,
            update: (0..update_len).map(|i| (i * 7) as u8).collect(),
        }
    }

    fn sealed(update_len: usize, seed: u64) -> Vec<u8> {
        seal_presence(
            &header(),
            &body(update_len),
            &presence_key(),
            &device_key(),
            &kb_types::testing::SeededRng::new(seed),
        )
    }

    #[test]
    fn a_sealed_presence_opens_with_its_key_and_the_senders_verifier() {
        let bytes = sealed(300, 1);
        assert_eq!(peek_presence(&bytes).unwrap(), header());
        let (opened_header, opened_body) =
            open_presence(&bytes, &presence_key(), &device_key().verifying_key()).unwrap();
        assert_eq!(opened_header, header());
        assert_eq!(opened_body, body(300));
        // The envelope is of kind Presence (10), signed, header first.
        assert_eq!(&bytes[..3], &[1, 1, 10]);
        assert_eq!(
            u32::from_le_bytes(bytes[3..7].try_into().unwrap()) as usize,
            PRESENCE_HEADER_LEN
        );
        assert_eq!(
            &bytes[7..7 + PRESENCE_HEADER_LEN],
            &encode_presence_header(&header())[..]
        );
    }

    #[test]
    fn every_state_and_the_no_page_document_round_trip() {
        for state in [
            PresenceState::Viewing,
            PresenceState::Editing,
            PresenceState::Idle,
        ] {
            let body = PresenceBody {
                doc_id: kb_types::sync::KEYRING_DOC_ID,
                state,
                update: Vec::new(),
            };
            let bytes = seal_presence(
                &header(),
                &body,
                &presence_key(),
                &device_key(),
                &kb_types::testing::SeededRng::new(5),
            );
            assert_eq!(
                open_presence(&bytes, &presence_key(), &device_key().verifying_key())
                    .unwrap()
                    .1,
                body
            );
        }
    }

    #[test]
    fn sealing_twice_gives_different_bytes_and_the_same_content() {
        let one = sealed(64, 1);
        let two = sealed(64, 2);
        assert_ne!(one, two);
        let verifier = device_key().verifying_key();
        assert_eq!(
            open_presence(&one, &presence_key(), &verifier).unwrap(),
            open_presence(&two, &presence_key(), &verifier).unwrap()
        );
    }

    #[test]
    fn the_signature_is_checked_before_anything_is_decrypted() {
        let bytes = sealed(64, 1);
        let other = SigningKey::from_seed(&Secret32::from_bytes([0x63; 32])).verifying_key();
        // A wrong verifier and a wrong key both fail authentication; a wrong verifier with a wrong key
        // fails on the signature (the key is never tried).
        assert_eq!(
            open_presence(&bytes, &presence_key(), &other),
            Err(SyncError::Authentication)
        );
        assert_eq!(
            open_presence(
                &bytes,
                &Secret32::from_bytes([0x64; 32]),
                &device_key().verifying_key()
            ),
            Err(SyncError::Authentication)
        );
        assert_eq!(
            open_presence(&bytes, &Secret32::from_bytes([0x64; 32]), &other),
            Err(SyncError::Authentication)
        );
        // One flipped byte anywhere past the envelope prefix (header, nonce, ciphertext, signature)
        // fails; so does a missing signature.
        let verifier = device_key().verifying_key();
        for i in 7..bytes.len() {
            let mut copy = bytes.clone();
            copy[i] ^= 1;
            assert!(
                open_presence(&copy, &presence_key(), &verifier).is_err(),
                "byte {i}"
            );
        }
        let unsigned = &bytes[..bytes.len() - kb_crypto::SIGNATURE_LEN];
        assert!(open_presence(unsigned, &presence_key(), &verifier).is_err());
    }

    #[test]
    fn a_presence_envelope_is_not_another_envelope_kind() {
        // An entry envelope under the presence key does not peek or open as presence.
        let entry = kb_crypto::seal_envelope(
            EnvelopeKind::Entry,
            &encode_presence_header(&header()),
            &presence_key(),
            b"x",
            Some(&device_key()),
            &kb_types::testing::SeededRng::new(3),
        );
        assert!(matches!(peek_presence(&entry), Err(SyncError::Encoding(_))));
        assert!(open_presence(&entry, &presence_key(), &device_key().verifying_key()).is_err());
        assert!(peek_presence(&[]).is_err());
        assert!(peek_presence(&[1, 1, 10]).is_err());
    }

    #[test]
    fn a_body_with_an_unknown_version_state_or_trailing_bytes_is_refused() {
        let key = presence_key();
        let verifier = device_key().verifying_key();
        let seal_raw = |plain: &[u8]| {
            let padded = kb_crypto::pad_to_bucket(plain, &kb_types::testing::SeededRng::new(9));
            kb_crypto::seal_envelope(
                EnvelopeKind::Presence,
                &encode_presence_header(&header()),
                &key,
                &padded,
                Some(&device_key()),
                &kb_types::testing::SeededRng::new(9),
            )
        };
        let mut good = vec![1u8];
        good.extend_from_slice(&[8; 16]);
        good.push(2);
        good.extend_from_slice(&3u32.to_le_bytes());
        good.extend_from_slice(b"abc");
        assert_eq!(
            open_presence(&seal_raw(&good), &key, &verifier)
                .unwrap()
                .1
                .update,
            b"abc"
        );
        let mut version = good.clone();
        version[0] = 2;
        let mut state = good.clone();
        state[17] = 0;
        let mut state_high = good.clone();
        state_high[17] = 4;
        let mut trailing = good.clone();
        trailing.push(0);
        let mut truncated = good.clone();
        truncated.truncate(good.len() - 1);
        for bad in [version, state, state_high, trailing, truncated, Vec::new()] {
            assert!(matches!(
                open_presence(&seal_raw(&bad), &key, &verifier),
                Err(SyncError::Encoding(_))
            ));
        }
    }

    #[test]
    fn a_full_size_update_fits_a_presence_frame() {
        let update = kb_types::sharing::PRESENCE_UPDATE_MAX_BYTES;
        let envelope = sealed(update, 4);
        let frame = PresenceFrame {
            feed: header().feed,
            envelope,
        };
        let bytes = encode_presence_frame(&frame).unwrap();
        assert!(
            bytes.len() <= PRESENCE_MAX_FRAME_BYTES,
            "{} bytes",
            bytes.len()
        );
        let ClientFrame::Presence(back) = decode_client_binary(&bytes).unwrap() else {
            panic!("a presence frame");
        };
        assert_eq!(
            open_presence(
                &back.envelope,
                &presence_key(),
                &device_key().verifying_key()
            )
            .unwrap()
            .1
            .update
            .len(),
            update
        );
    }

    // ----- the frame limiter -----------------------------------------------------------------

    #[test]
    fn the_limiter_lets_ten_frames_through_per_second_and_drops_the_rest() {
        let mut limiter = FrameLimiter::presence();
        for _ in 0..10 {
            assert!(limiter.allow(1_000));
        }
        assert!(!limiter.allow(1_000));
        assert!(!limiter.allow(1_999));
        // The frames leave the window exactly one second after they came: ten more pass.
        for _ in 0..10 {
            assert!(limiter.allow(2_000));
        }
        assert!(!limiter.allow(2_000));
    }

    #[test]
    fn the_limiter_slides_with_the_frames_not_with_the_wall_clock() {
        let mut limiter = FrameLimiter::new(3, 1_000);
        assert!(limiter.allow(0));
        assert!(limiter.allow(400));
        assert!(limiter.allow(800));
        assert!(!limiter.allow(900));
        assert!(limiter.allow(1_000));
        assert!(!limiter.allow(1_100));
        assert!(limiter.allow(1_400));
        // 50 frames in one second: the first 3 pass.
        let mut burst = FrameLimiter::new(3, 1_000);
        let passed = (0..50).filter(|i| burst.allow(5_000 + i * 10)).count();
        assert_eq!(passed, 3);
    }

    #[test]
    fn the_limiter_survives_a_clock_that_goes_back_and_a_zero_limit() {
        let mut limiter = FrameLimiter::new(2, 1_000);
        assert!(limiter.allow(10_000));
        assert!(limiter.allow(10_000));
        assert!(!limiter.allow(5_000));
        let mut nothing = FrameLimiter::new(0, 1_000);
        assert!(!nothing.allow(0));
    }
}
