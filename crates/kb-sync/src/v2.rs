//! Sync protocol v2 (M13, ADR-0045, `docs/security/sharing.md` sections 5 and 6,
//! `docs/development/milestone-13.md` section 1.3): feeds (a personal one and one per shared space),
//! the v2 headers, the [`RemoteStore`] every server implements, the reference [`MemoryRemote`] with an
//! access list, and the conformance suite (`conformance::run_all_v2`, behind the `testing` feature).
//!
//! **v2 replaces v1 without a server migration**: protocol v1 is retired, and its headers, remote,
//! reference remote and conformance suite are gone, and `kb_sync::v2` stays the path of the API
//! (the crate root keeps what both versions shared: the bodies, the pages, the authentication
//! message, the chain hash). `kb_types::sync::PROTOCOL_VERSION` is 2; a v1 object (header version
//! byte 1) is refused with `SyncError::Protocol` by every decoder here.
//!
//! # Formats (integers little-endian, ids 16 raw bytes)
//!
//! ```text
//! entry        u8 2 | feed | account | doc | device | u64 device_seq | u32 key_epoch      (77)
//! keyring      u8 2 | feed | account | device | u64 device_seq | u32 key_epoch            (61)
//! snapshot     u8 2 | feed | account | doc | device | u64 covers_cursor | u32 key_epoch   (77)
//! membership   u8 2 | feed | u64 seq | [32] prev | account | device | u8 kind | fields | i64 at | u32 meta_epoch
//!              (`kb_keys::membership::MembershipHeader`)
//! head         u8 2 | feed | account | device | u64 cursor | [32] chain | [16] server_epoch | i64 at   (113)
//!              || [64] Ed25519 signature, context "head"                                              (177)
//! presence     u8 2 | feed | account | device | u32 epoch | u64 counter                              (61)
//!              (`ws::PresenceHeader`; the envelope of kind `Presence` of a WebSocket frame, never stored)
//! publication  u8 2 | link id                                                                  (17)
//!              (`publication::PublicationHeader`; the envelope of kind `Publication`, unsigned, sealed under
//!              the link key; the server stores it as it is and serves it to whoever holds the link id)
//! ```
//!
//! The frames of `GET /v2/ws` (subscribe, `changed`, inline entries, presence) are in [`ws`] (M13 slice 3).
//! View links (the publication, the upload body and the answers of `GET /v2/links/{id}`) are in
//! [`publication`] (M13 slice 4); the four calls are the `link_*` methods of [`RemoteStore`].
//!
//! The bodies are those of the crate root (`EntryBody`, `SnapshotBody`, the keyring body): envelopes of kind
//! `Entry`, `Keyring`, `Snapshot` and `Head`, bucket-padded and signed by the device exactly as in
//! `crate`'s documentation; a membership entry is one envelope of kind `Membership`
//! (`kb_keys::membership`). Personal keyring entries carry the ARK generation as `key_epoch`; a
//! shared feed's keyring entries are sealed under `kb_keys::space_keys::space_keyring_key` with the
//! space key epoch as `key_epoch`. A feed's chain hash `h_n` (genesis `BLAKE3(server_epoch)`, then
//! `chain_next` over every item's bytes, membership entries included), its cursors (filtered from
//! one global autoincrement), its heads (feed, device), snapshots (feed, doc) and chunks (feed, hash)
//! work as ADR-0021 described them, inside each feed.
//!
//! # What the remote does (the semantics `MemoryRemote` and `kb-server` share)
//!
//! - **Access** of the session's account to a feed: the personal feed (id = the account id) belongs
//!   to that account's active devices; a shared feed answers by the folded membership log. A
//!   **stranger** (never a member, or an unknown feed) gets [`SyncError::NotFound`] everywhere. A
//!   **member** needs: Commenter for appends, snapshot puts and snapshot disputes (Editor for
//!   disputes), Editor for chunk puts and copies, Owner for chunk deletes, any role for reads and
//!   heads; a role that is too low is [`SyncError::Forbidden`]. A **removed** (or departed) account
//!   reads the membership log up to and including its own `Remove`/`Leave`, and the feed's items up
//!   to that cursor (`changes`), and nothing else: `changes` at or past it is
//!   [`SyncError::Forbidden`], every other call is `NotFound`.
//! - **Content appends** (entries and keyring entries) are refused with [`SyncError::RekeyNeeded`]
//!   while the feed waits for a `Rekey` (after a `Leave` or a device revocation in a member's
//!   account), with [`SyncError::StaleEpoch`] when the `key_epoch` is below the feed's epoch, and
//!   with [`SyncError::Closed`] after a `Close` (which also refuses every other write).
//! - **Membership** goes through [`RemoteStore::membership_append`] (and the `Create` through
//!   [`RemoteStore::feeds_create`]): the server runs [`kb_keys::membership::Timeline::apply`] on its
//!   own copy of the device logs, plus the checks only it can make (see [`guard`]).
//! - **View links** (M13 slice 4, ADR-0051): `link_put` needs a session that is an Editor or Owner of
//!   the feed named in the upload (the account itself for its personal feed); a stranger gets
//!   [`SyncError::NotFound`], a Viewer or Commenter [`SyncError::Forbidden`], a closed feed
//!   [`SyncError::Closed`]. The first upload of a link id creates the row (`created_by` = the
//!   session's account, `expires_at` = the upload's, `pub_seq` = 1); later uploads need the same
//!   feed ([`SyncError::Conflict`] otherwise), keep the stored creator and expiry, replace the
//!   publication and the declared chunks and add 1 to `pub_seq`. `link_get` and `link_chunk_get`
//!   need no session and answer one [`SyncError::NotFound`] for a link that is unknown, expired (the
//!   server's clock) or revoked, and for a chunk that is not declared or that the feed does not
//!   hold; past `LINK_GETS_PER_MINUTE_PER_IP` reads a minute they answer
//!   [`SyncError::Unavailable`]. `link_delete` needs a session that is the link's creator (while
//!   still a member) or an Owner of the link's feed; it removes the row, the publication and the
//!   declared chunks. Closing a feed deletes its links.
//! - **Errors** on the wire (HTTP) map to [`SyncError`] as `milestone-13.md` 1.1 lists them.
//!
//! The synchronous `<method>_sync` twins of [`MemoryRemote`] answer exactly what the trait does.

use kb_crypto::{
    Envelope, EnvelopeKind, SIGNATURE_LEN, Secret32, Signature, SigningKey, VerifyingKey,
};
use kb_keys::KeyringItem;
use kb_keys::membership::MembershipEntry;
use kb_types::sync::{FeedKind, KEYRING_DOC_ID, PROTOCOL_VERSION};
use kb_types::{AccountId, ChunkId, DeviceId, DocId, FeedId, InviteId, LinkId, MemberRole, Rng};

use crate::codec::Reader;
use crate::{
    ChangesPage, EntryBody, PairingPage, SnapshotBody, SnapshotsPage, SyncError, check_signature,
    crypto_error, decode_entry_body, decode_envelope, decode_keyring_body, decode_snapshot_body,
    encode_entry_body, encode_keyring_body, encode_snapshot_body, open_body, read_account,
    read_device, read_doc, seal_body,
};

#[cfg(any(test, feature = "testing"))]
pub mod conformance;
pub mod guard;
pub mod host;
pub mod memory;
pub mod publication;
#[cfg(test)]
mod tests;
pub mod ws;

pub use host::{
    FeedChunkKind, FeedChunkWork, FeedWork, InlineOutcome, InviteOpenRequest, InviteSignup,
    LinkOutcome, LinkWork, OutboundEntry, PendingMembership, SealedPresence, SnapshotsApplied,
    SyncApplied,
};
pub use kb_keys::membership::{ChainHead as MembershipHead, MembershipHeader};
pub use memory::{MemoryRemote, MemorySocket};
pub use publication::{
    LinkFetch, LinkKey, LinkPut, Publication, PublicationHeader, PublishedAttachment,
    PublishedPerson, decode_link_put, decode_publication, encode_link_put, encode_publication,
    encode_publication_header, open_publication, peek_publication, seal_publication,
};
pub use ws::{
    ClientFrame, FrameLimiter, InlineEntry, PresenceBody, PresenceFrame, PresenceHeader,
    ServerFrame, decode_client_binary, decode_client_text, decode_server_binary,
    decode_server_text, encode_changed, encode_entry_frame, encode_presence_frame,
    encode_presence_header, encode_subscribe, open_presence, peek_presence, seal_presence,
};

/// Bytes of an entry header.
pub const ENTRY_HEADER_LEN: usize = 1 + 16 + 16 + 16 + 16 + 8 + 4;
/// Bytes of a keyring entry header.
pub const KEYRING_HEADER_LEN: usize = 1 + 16 + 16 + 16 + 8 + 4;
/// Bytes of a snapshot header.
pub const SNAPSHOT_HEADER_LEN: usize = 1 + 16 + 16 + 16 + 16 + 8 + 4;
/// Bytes of a head record without its signature.
pub const HEAD_RECORD_LEN: usize = 1 + 16 + 16 + 16 + 8 + 32 + 16 + 8;
/// Bytes of a signed head ([`sign_head`] output).
pub const SIGNED_HEAD_LEN: usize = HEAD_RECORD_LEN + kb_crypto::SIGNATURE_LEN;
/// Signature context of heads.
pub const HEAD_CONTEXT: &str = "head";
/// The role byte of the opener of a pairing mailbox (`pair_post`): the joiner of a device pairing,
/// the inviter of an invite.
pub const PAIR_ROLE_OPENER: u8 = crate::PAIRING_ROLE_JOINER;
/// The role byte of the typist of a pairing mailbox: the sponsor of a device pairing, the invitee of
/// an invite.
pub const PAIR_ROLE_TYPIST: u8 = crate::PAIRING_ROLE_SPONSOR;

/// The version byte of v1 headers: refused with [`SyncError::Protocol`].
const V1_VERSION: u8 = 1;

// ----- headers ------------------------------------------------------------------------------

/// Plaintext header of a document entry (what the server sees).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryHeader {
    /// The feed that carries the entry.
    pub feed: FeedId,
    /// The account that wrote it (a device is resolved only inside this account's log).
    pub account_id: AccountId,
    /// The document the updates belong to.
    pub doc_id: DocId,
    /// The device that wrote and signed the entry.
    pub device_id: DeviceId,
    /// Per `(feed, doc_id, device_id)` sequence, from 1 without gaps.
    pub device_seq: u64,
    /// Epoch of the document key `DK[doc_id][key_epoch]` that encrypts the body (the feed's epoch or
    /// below; the server refuses an epoch below the feed's: `SyncError::StaleEpoch`).
    pub key_epoch: u32,
}

/// Plaintext header of a keyring entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyringHeader {
    /// The feed that carries the entry.
    pub feed: FeedId,
    /// The account that wrote it.
    pub account_id: AccountId,
    /// The device that wrote and signed the entry.
    pub device_id: DeviceId,
    /// Per-device sequence of keyring entries in this feed (feed document `KEYRING_DOC_ID`).
    pub device_seq: u64,
    /// The ARK generation (personal feed) or the space key epoch (shared feed) the body is sealed
    /// under.
    pub key_epoch: u32,
}

/// Plaintext header of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotHeader {
    /// The feed.
    pub feed: FeedId,
    /// The account that made it.
    pub account_id: AccountId,
    /// The document.
    pub doc_id: DocId,
    /// The device that made and signed the snapshot.
    pub device_id: DeviceId,
    /// The latest cursor whose entries for this document the snapshot includes (checked by the
    /// members' devices: snapshot dominance, ADR-0045).
    pub covers_cursor: u64,
    /// Epoch of the document key that encrypts the body.
    pub key_epoch: u32,
}

/// A device's signed statement of where it stands in a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// The feed.
    pub feed: FeedId,
    /// The account.
    pub account_id: AccountId,
    /// The device that signs the head.
    pub device_id: DeviceId,
    /// The last cursor of the feed the device has applied.
    pub cursor: u64,
    /// The chain hash after that cursor.
    pub chain_hash: [u8; 32],
    /// The server epoch the cursor belongs to.
    pub server_epoch: [u8; 16],
    /// When the head was made, Unix ms (display only).
    pub at_ms: i64,
}

/// What kind of object a feed item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// A document entry (envelope kind `Entry`).
    Entry,
    /// A keyring entry (envelope kind `Keyring`).
    Keyring,
    /// A membership entry (envelope kind `Membership`).
    Membership,
}

/// The plaintext facts of a feed item, whatever its kind: what a server needs to route,
/// deduplicate and check it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedItemInfo {
    /// Entry, keyring or membership entry.
    pub kind: ItemKind,
    /// The feed named in the header.
    pub feed: FeedId,
    /// The account named in the header.
    pub account_id: AccountId,
    /// The document; `KEYRING_DOC_ID` (all zeros) for keyring and membership items.
    pub doc_id: DocId,
    /// The signing device.
    pub device_id: DeviceId,
    /// The device's sequence for this document or the keyring; the membership `seq` for a
    /// membership entry.
    pub device_seq: u64,
}

// ----- codec ---------------------------------------------------------------------------------
//
// The v1 functions of the crate root are the model: bucket padding, a device signature over the
// whole envelope, the signature checked before anything is decrypted; `Encoding` for malformed
// bytes, `Authentication` for a failed signature or tag. Differences: the headers above, and a
// header that carries the version byte of v1 is `SyncError::Protocol` (refused outright, not
// "malformed").

/// Reads and checks the leading protocol version of a v2 header.
fn read_version(r: &mut Reader<'_>) -> Result<(), SyncError> {
    match r.u8("protocol version")? {
        PROTOCOL_VERSION => Ok(()),
        V1_VERSION => Err(SyncError::Protocol(
            "a protocol v1 object is not accepted by protocol v2".to_owned(),
        )),
        other => Err(SyncError::Encoding(format!(
            "unsupported protocol version {other}"
        ))),
    }
}

fn read_feed(r: &mut Reader<'_>) -> Result<FeedId, SyncError> {
    r.array("feed id").map(FeedId::from_bytes)
}

/// The header bytes of an entry ([`ENTRY_HEADER_LEN`]); the envelope header of [`seal_entry`].
#[must_use]
pub fn encode_entry_header(header: &EntryHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(ENTRY_HEADER_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(header.feed.as_bytes());
    out.extend_from_slice(header.account_id.as_bytes());
    out.extend_from_slice(header.doc_id.as_bytes());
    out.extend_from_slice(header.device_id.as_bytes());
    out.extend_from_slice(&header.device_seq.to_le_bytes());
    out.extend_from_slice(&header.key_epoch.to_le_bytes());
    out
}

fn decode_entry_header(bytes: &[u8]) -> Result<EntryHeader, SyncError> {
    let mut r = Reader::new(bytes, "entry header");
    read_version(&mut r)?;
    let header = EntryHeader {
        feed: read_feed(&mut r)?,
        account_id: read_account(&mut r)?,
        doc_id: read_doc(&mut r)?,
        device_id: read_device(&mut r)?,
        device_seq: r.u64("device sequence")?,
        key_epoch: r.u32("key epoch")?,
    };
    r.finish()?;
    Ok(header)
}

/// Seals a document entry under its document key, signed by the writer device (`header` in the
/// clear, the body bucket-padded and encrypted). A body that cannot be represented gives an empty
/// result, which every decoder refuses.
#[must_use]
pub fn seal_entry(
    header: &EntryHeader,
    body: &EntryBody,
    doc_key: &Secret32,
    signer: &SigningKey,
    rng: &dyn Rng,
) -> Vec<u8> {
    seal_body(
        EnvelopeKind::Entry,
        &encode_entry_header(header),
        encode_entry_body(body),
        doc_key,
        signer,
        rng,
    )
}

/// Reads the header of an entry without a key or a signature check (what a server routes on).
pub fn peek_entry(bytes: &[u8]) -> Result<EntryHeader, SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Entry)?;
    decode_entry_header(&envelope.header)
}

/// Verifies the signature, then decrypts an entry. The signature is checked before anything is
/// decrypted.
pub fn open_entry(
    bytes: &[u8],
    doc_key: &Secret32,
    verifier: &VerifyingKey,
) -> Result<(EntryHeader, EntryBody), SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Entry)?;
    let header = decode_entry_header(&envelope.header)?;
    let content = open_body(bytes, &envelope, doc_key, verifier)?;
    Ok((header, decode_entry_body(&content)?))
}

/// The header bytes of a keyring entry ([`KEYRING_HEADER_LEN`]).
#[must_use]
pub fn encode_keyring_header(header: &KeyringHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(KEYRING_HEADER_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(header.feed.as_bytes());
    out.extend_from_slice(header.account_id.as_bytes());
    out.extend_from_slice(header.device_id.as_bytes());
    out.extend_from_slice(&header.device_seq.to_le_bytes());
    out.extend_from_slice(&header.key_epoch.to_le_bytes());
    out
}

fn decode_keyring_header(bytes: &[u8]) -> Result<KeyringHeader, SyncError> {
    let mut r = Reader::new(bytes, "keyring header");
    read_version(&mut r)?;
    let header = KeyringHeader {
        feed: read_feed(&mut r)?,
        account_id: read_account(&mut r)?,
        device_id: read_device(&mut r)?,
        device_seq: r.u64("device sequence")?,
        key_epoch: r.u32("key epoch")?,
    };
    r.finish()?;
    Ok(header)
}

/// Seals keyring items for a feed: under `HKDF(ARK, "keyring")` ([`crate::keyring_key`]) for the
/// personal feed, under `kb_keys::space_keys::space_keyring_key` for a shared feed. The items keep
/// their own signatures, which [`open_keyring`] does not check (the core resolves each item's
/// signer through the device log).
#[must_use]
pub fn seal_keyring(
    header: &KeyringHeader,
    items: &[KeyringItem],
    keyring_key: &Secret32,
    signer: &SigningKey,
    rng: &dyn Rng,
) -> Vec<u8> {
    seal_body(
        EnvelopeKind::Keyring,
        &encode_keyring_header(header),
        encode_keyring_body(items),
        keyring_key,
        signer,
        rng,
    )
}

/// Reads the header of a keyring entry without a key.
pub fn peek_keyring(bytes: &[u8]) -> Result<KeyringHeader, SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Keyring)?;
    decode_keyring_header(&envelope.header)
}

/// Verifies the signature, then decrypts a keyring entry. Each item is decoded strictly; its own
/// signature is left to the caller (`KeyringItem::verify`).
pub fn open_keyring(
    bytes: &[u8],
    keyring_key: &Secret32,
    verifier: &VerifyingKey,
) -> Result<(KeyringHeader, Vec<KeyringItem>), SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Keyring)?;
    let header = decode_keyring_header(&envelope.header)?;
    let content = open_body(bytes, &envelope, keyring_key, verifier)?;
    Ok((header, decode_keyring_body(&content)?))
}

/// The header bytes of a snapshot ([`SNAPSHOT_HEADER_LEN`]).
#[must_use]
pub fn encode_snapshot_header(header: &SnapshotHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(SNAPSHOT_HEADER_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(header.feed.as_bytes());
    out.extend_from_slice(header.account_id.as_bytes());
    out.extend_from_slice(header.doc_id.as_bytes());
    out.extend_from_slice(header.device_id.as_bytes());
    out.extend_from_slice(&header.covers_cursor.to_le_bytes());
    out.extend_from_slice(&header.key_epoch.to_le_bytes());
    out
}

fn decode_snapshot_header(bytes: &[u8]) -> Result<SnapshotHeader, SyncError> {
    let mut r = Reader::new(bytes, "snapshot header");
    read_version(&mut r)?;
    let header = SnapshotHeader {
        feed: read_feed(&mut r)?,
        account_id: read_account(&mut r)?,
        doc_id: read_doc(&mut r)?,
        device_id: read_device(&mut r)?,
        covers_cursor: r.u64("covered cursor")?,
        key_epoch: r.u32("key epoch")?,
    };
    r.finish()?;
    Ok(header)
}

/// Seals a snapshot under its document key, signed by the device that made it.
#[must_use]
pub fn seal_snapshot(
    header: &SnapshotHeader,
    body: &SnapshotBody,
    doc_key: &Secret32,
    signer: &SigningKey,
    rng: &dyn Rng,
) -> Vec<u8> {
    seal_body(
        EnvelopeKind::Snapshot,
        &encode_snapshot_header(header),
        encode_snapshot_body(body),
        doc_key,
        signer,
        rng,
    )
}

/// Reads the header of a snapshot without a key.
pub fn peek_snapshot(bytes: &[u8]) -> Result<SnapshotHeader, SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Snapshot)?;
    decode_snapshot_header(&envelope.header)
}

/// Verifies the signature, then decrypts a snapshot.
pub fn open_snapshot(
    bytes: &[u8],
    doc_key: &Secret32,
    verifier: &VerifyingKey,
) -> Result<(SnapshotHeader, SnapshotBody), SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Snapshot)?;
    let header = decode_snapshot_header(&envelope.header)?;
    let content = open_body(bytes, &envelope, doc_key, verifier)?;
    Ok((header, decode_snapshot_body(&content)?))
}

/// Reads the clear header of a membership entry (`MembershipEntry::peek`), mapping its errors to
/// [`SyncError::Encoding`]. Verifies nothing.
pub fn peek_membership(bytes: &[u8]) -> Result<MembershipHeader, SyncError> {
    MembershipEntry::peek(bytes).map_err(|error| SyncError::Encoding(error.to_string()))
}

/// Verifies only the device signature of any sealed object (entry, keyring entry, snapshot,
/// membership entry) against `verifier`: what a server checks before it stores (it holds no key). A
/// missing or invalid signature is [`SyncError::Authentication`].
pub fn verify_signature(bytes: &[u8], verifier: &VerifyingKey) -> Result<(), SyncError> {
    let envelope = Envelope::decode(bytes).map_err(crypto_error)?;
    check_signature(bytes, &envelope, verifier)
}

fn encode_head(head: &Head) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIGNED_HEAD_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(head.feed.as_bytes());
    out.extend_from_slice(head.account_id.as_bytes());
    out.extend_from_slice(head.device_id.as_bytes());
    out.extend_from_slice(&head.cursor.to_le_bytes());
    out.extend_from_slice(&head.chain_hash);
    out.extend_from_slice(&head.server_epoch);
    out.extend_from_slice(&head.at_ms.to_le_bytes());
    out
}

/// Splits a signed head into its record (decoded) and signature.
fn decode_head(bytes: &[u8]) -> Result<(Head, &[u8], Signature), SyncError> {
    let mut r = Reader::new(bytes, "head");
    let record_bytes = bytes.get(..HEAD_RECORD_LEN).unwrap_or(bytes);
    read_version(&mut r)?;
    let head = Head {
        feed: read_feed(&mut r)?,
        account_id: read_account(&mut r)?,
        device_id: read_device(&mut r)?,
        cursor: r.u64("cursor")?,
        chain_hash: r.array("chain hash")?,
        server_epoch: r.array("server epoch")?,
        at_ms: r.i64("time")?,
    };
    let signature =
        Signature::from_bytes(&r.array::<SIGNATURE_LEN>("signature")?).map_err(crypto_error)?;
    r.finish()?;
    Ok((head, record_bytes, signature))
}

/// Signs a head: the record followed by the Ed25519 signature (context [`HEAD_CONTEXT`]);
/// [`SIGNED_HEAD_LEN`] bytes.
#[must_use]
pub fn sign_head(head: &Head, signer: &SigningKey) -> Vec<u8> {
    let mut out = encode_head(head);
    let signature = kb_crypto::sign(signer, HEAD_CONTEXT, &out);
    out.extend_from_slice(&signature.to_bytes());
    out
}

/// Reads a signed head without checking the signature (to find the signing device).
pub fn peek_head(bytes: &[u8]) -> Result<Head, SyncError> {
    decode_head(bytes).map(|(head, _, _)| head)
}

/// Reads a signed head and checks its signature against the device's key.
pub fn verify_head(bytes: &[u8], verifier: &VerifyingKey) -> Result<Head, SyncError> {
    let (head, record, signature) = decode_head(bytes)?;
    kb_crypto::verify(verifier, HEAD_CONTEXT, record, &signature)
        .map_err(|_| SyncError::Authentication)?;
    Ok(head)
}

/// What kind of feed item the bytes are, from the envelope's version, suite and kind bytes: `Entry`,
/// `Keyring` or `Membership`; any other kind is [`SyncError::Encoding`].
pub fn feed_kind(bytes: &[u8]) -> Result<ItemKind, SyncError> {
    let [version, suite, kind, ..] = bytes else {
        return Err(SyncError::Encoding(
            "feed item: truncated envelope prefix".to_owned(),
        ));
    };
    if *version != kb_crypto::ENVELOPE_VERSION {
        return Err(SyncError::Encoding(format!(
            "feed item: unknown envelope version {version}"
        )));
    }
    if *suite != kb_crypto::SUITE_ID {
        return Err(SyncError::Encoding(format!(
            "feed item: unknown suite {suite}"
        )));
    }
    match EnvelopeKind::from_byte(*kind) {
        Some(EnvelopeKind::Entry) => Ok(ItemKind::Entry),
        Some(EnvelopeKind::Keyring) => Ok(ItemKind::Keyring),
        Some(EnvelopeKind::Membership) => Ok(ItemKind::Membership),
        Some(other) => Err(SyncError::Encoding(format!(
            "feed item: an {} envelope is not a feed item",
            other.context()
        ))),
        None => Err(SyncError::Encoding(format!(
            "feed item: unknown envelope kind {kind}"
        ))),
    }
}

/// [`feed_kind`], then the matching `peek_*`: the routing and deduplication facts of a feed item
/// (keyring and membership items report `KEYRING_DOC_ID`; a membership item's `device_seq` is its
/// `seq`).
pub fn peek_feed_item(bytes: &[u8]) -> Result<FeedItemInfo, SyncError> {
    match feed_kind(bytes)? {
        ItemKind::Entry => peek_entry(bytes).map(|h| FeedItemInfo {
            kind: ItemKind::Entry,
            feed: h.feed,
            account_id: h.account_id,
            doc_id: h.doc_id,
            device_id: h.device_id,
            device_seq: h.device_seq,
        }),
        ItemKind::Keyring => peek_keyring(bytes).map(|h| FeedItemInfo {
            kind: ItemKind::Keyring,
            feed: h.feed,
            account_id: h.account_id,
            doc_id: KEYRING_DOC_ID,
            device_id: h.device_id,
            device_seq: h.device_seq,
        }),
        ItemKind::Membership => peek_membership(bytes).map(|h| FeedItemInfo {
            kind: ItemKind::Membership,
            feed: h.feed,
            account_id: h.account,
            doc_id: KEYRING_DOC_ID,
            device_id: h.device,
            device_seq: h.seq,
        }),
    }
}

/// The chain hash of an empty feed: `BLAKE3(server_epoch)` (as in v1).
#[must_use]
pub fn chain_genesis(server_epoch: &[u8; 16]) -> [u8; 32] {
    crate::chain_genesis(server_epoch)
}

/// The chain hash after one more item: `BLAKE3(prev || bytes)` (as in v1).
#[must_use]
pub fn chain_next(prev: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
    crate::chain_next(prev, bytes)
}
// ----- answers of the remote -----------------------------------------------------------------

/// What `GET /v2/server` answers (no session needed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// The protocol version the server speaks (2).
    pub protocol_version: u8,
    /// The server's storage epoch.
    pub server_epoch: [u8; 16],
    /// The largest entry, snapshot or chunk the server accepts.
    pub max_wire_bytes: u64,
    /// The per-account storage quota, if the operator set one.
    pub account_quota_bytes: Option<u64>,
    /// Devices idle for more days than this are refused, if the operator set a limit.
    pub idle_device_days: Option<u32>,
    /// Accounts share invites may create on this server (`KB_SERVER_SHARE_SIGNUPS`; 0 = none).
    pub share_signups: u32,
    /// Accounts one inviting account's invites may create (`KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT`).
    pub share_signups_per_account: u32,
}

/// A feed as `GET /v2/feeds` lists it for the session's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedListing {
    /// The feed.
    pub feed: FeedId,
    /// Personal or shared.
    pub kind: FeedKind,
    /// The account's role in a shared feed (the server's materialised view of the log); `None` for
    /// the personal feed and for a removed account.
    pub role: Option<MemberRole>,
    /// The account was removed (it may still read the membership log up to its `Remove`).
    pub removed: bool,
    /// An Owner closed the feed.
    pub closed: bool,
}

/// What `feeds_create` answers: the feed exists and its membership log starts with the `Create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedCreated {
    /// The feed.
    pub feed: FeedId,
    /// The membership head after the `Create` (`seq` 0).
    pub head: MembershipHead,
    /// The `Create`'s cursor in the feed.
    pub cursor: u64,
}

/// A membership entry with the feed cursor it sits at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipItem {
    /// The cursor.
    pub cursor: u64,
    /// `MembershipEntry::encode`.
    pub bytes: Vec<u8>,
}

/// A page of the membership log (`membership_get`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipPage {
    /// Entries with `seq > after`, in order.
    pub entries: Vec<MembershipItem>,
    /// The head of the whole log (a removed account sees the head at its `Remove`).
    pub head: MembershipHead,
    /// More entries follow.
    pub more: bool,
}

/// A new snapshot of a feed, as `snapshot_marks` announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMark {
    /// The mark's position in the feed's snapshot order: the `after` of the next call.
    pub mark: u64,
    /// The document.
    pub doc_id: DocId,
    /// The device that published the snapshot.
    pub device_id: DeviceId,
    /// Its account.
    pub account_id: AccountId,
    /// The cursor the snapshot covers.
    pub covers_cursor: u64,
}

/// A page of snapshot marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarksPage {
    /// Marks with `mark > after`, in order.
    pub marks: Vec<SnapshotMark>,
    /// More follow.
    pub more: bool,
}

/// What `invites/{id}/open` answers: the feed and its membership log, so the invitee can verify
/// that an Owner wrote the `Invite` before it joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteOpened {
    /// The feed the invite is for.
    pub feed: FeedId,
    /// The whole membership log with cursors.
    pub log: Vec<MembershipItem>,
}

/// One account's device log to fetch in a batch (`logs_batch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRequest {
    /// The account.
    pub account: AccountId,
    /// The last `seq` the caller holds; `None` asks for the whole log.
    pub after: Option<u64>,
}

/// One account's answer in a batch: the encoded log entries after the requested `seq`, in order. An
/// account the server does not know is left out of the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogChunk {
    /// The account.
    pub account: AccountId,
    /// `LogEntry::encode` bytes.
    pub entries: Vec<Vec<u8>>,
}

// ----- the remote ----------------------------------------------------------------------------

/// A sync server (or a dumb store) as the client sees it, protocol v2 (ADR-0045). Bytes in and out
/// are the sealed objects above; the remote never needs a key. Every feed method takes the
/// [`FeedId`]; the server checks the session's account against the feed's access list on every call
/// (see the [module documentation](self) for the whole table):
///
/// - a **stranger** (an unknown feed, an account that never was a member) gets
///   [`SyncError::NotFound`], a member whose role is too low [`SyncError::Forbidden`]; a removed
///   account reads the membership log up to its own `Remove` and the feed up to that cursor, and
///   nothing else (`changes` past it is `Forbidden`, every other call `NotFound`);
/// - appends need Commenter and up (Editor and up for content documents is enforced by clients),
///   snapshot puts Commenter and up, snapshot disputes Editor and up, chunk puts and copies Editor
///   and up, chunk deletes an Owner, reads and heads any member;
/// - an append while `pending_rekey` is set is [`SyncError::RekeyNeeded`], one whose `key_epoch` is
///   below the feed's [`SyncError::StaleEpoch`], any write to a closed feed [`SyncError::Closed`], a
///   lost compare-and-swap or an id collision [`SyncError::Conflict`];
/// - `server_info`, `create_account`, `account_create_share`, `append_log`, `fetch_log`,
///   `logs_batch`, `invite_open`, the pairing mailbox (`pair_*`) and `recovery_get` need no session;
///   every other method needs one opened by `authenticate` ([`SyncError::Unauthorized`] otherwise).
///
/// `async fn` in a public trait, without `Send` bounds, exactly like v1 (it must work on wasm32);
/// `kb-net` boxes the futures. Not object-safe: code that works with any remote is generic over
/// `R: RemoteStore`.
#[allow(async_fn_in_trait)]
pub trait RemoteStore {
    // ---- server, accounts, logs, authentication ----

    /// Protocol version, server epoch and limits (no session needed).
    async fn server_info(&self) -> Result<ServerInfo, SyncError>;

    /// Creates the account from its device log (encoded entries, genesis first) with an operator
    /// invite token. An unknown token is [`SyncError::Unauthorized`], an existing account (or a
    /// device id another account uses) [`SyncError::Conflict`].
    async fn create_account(&self, invite_token: &str, log: &[Vec<u8>]) -> Result<(), SyncError>;

    /// Creates the account through a share invite instead of an operator token (`POST /v2/accounts`
    /// with `share {invite_id, sig}`; `sig` is `kb_keys::invite::sign_signup`). The invite must be
    /// open and unexpired and the signature must verify under its public key; a bound invite only
    /// admits its account; an invite creates at most one account; beyond `KB_SERVER_SHARE_SIGNUPS`
    /// accounts (or the per-inviter limit), or for a second signup with the same invite:
    /// [`SyncError::Forbidden`] (403 `signup_limit`). A dead invite is [`SyncError::NotFound`] (one
    /// answer for all), a bad signature [`SyncError::Authentication`].
    async fn account_create_share(
        &self,
        log: &[Vec<u8>],
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<(), SyncError>;

    /// Appends encoded device log entries; they must continue the stored chain (no session). A
    /// revocation in the log of a member of a shared feed sets that feed's `pending_rekey`.
    async fn append_log(&self, account: AccountId, entries: &[Vec<u8>]) -> Result<(), SyncError>;

    /// The stored device log of any account, encoded entries with `seq > after` in order (the
    /// whole log when `None`; no session: co-members fetch each other's logs). An unknown account
    /// is [`SyncError::NotFound`].
    async fn fetch_log(
        &self,
        account: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<Vec<u8>>, SyncError>;

    /// Several accounts' logs in one call (`POST /v2/accounts/logs`, no session). Accounts the
    /// server does not know are left out of the answer.
    async fn logs_batch(&self, requests: &[LogRequest]) -> Result<Vec<LogChunk>, SyncError>;

    /// Challenge and session: `sign` signs the [`crate::auth_message`]; the session token stays
    /// inside the remote. A later call replaces the session (one connection, one device at a time).
    async fn authenticate(
        &self,
        account: AccountId,
        device: DeviceId,
        sign: &dyn Fn(&[u8]) -> Signature,
    ) -> Result<(), SyncError>;

    // ---- feeds and membership ----

    /// Creates a shared feed from its `Create` (`MembershipEntry::encode`; the entry carries the
    /// feed id, which the server refuses on a collision, also with an account id, with
    /// [`SyncError::Conflict`]). The server folds the entry (a bad signature is
    /// [`SyncError::Authentication`], a rule or completeness violation [`SyncError::Protocol`]);
    /// at most `MAX_FEEDS_PER_ACCOUNT` feeds per creator (`Forbidden` beyond); the creator (the
    /// entry's signer) is the billing account. Any session may post a verified `Create`, so a member
    /// can re-post the log after a server reset (`sharing.md` section 11). Answers the feed, the
    /// membership head and the `Create`'s cursor.
    async fn feeds_create(&self, create: &[u8]) -> Result<FeedCreated, SyncError>;

    /// The feeds the session's account belongs to or belonged to: the personal feed and the shared
    /// feeds with its role (`None` once removed), whether it was removed and whether the feed is
    /// closed.
    async fn feeds_list(&self) -> Result<Vec<FeedListing>, SyncError>;

    /// Appends membership entries (`MembershipEntry::encode`, in log order) as a compare-and-swap:
    /// the first must have `seq == head_seq + 1` and `prev` equal to the head hash, otherwise
    /// [`SyncError::Conflict`]. The server runs `kb_keys::membership::Timeline::apply` over its copy
    /// of the device logs and the checks only it can make ([`guard`]); a batch is atomic (all or
    /// none). Errors for a **member**: a role violation [`SyncError::Forbidden`], an entry after a
    /// `Close` [`SyncError::Closed`], a bad signature [`SyncError::Authentication`], a device log the
    /// entry names that the server lacks (publish it first) [`SyncError::Conflict`], the Rekey rate
    /// limit [`SyncError::Unavailable`], any other malformed or rule-breaking entry
    /// [`SyncError::Protocol`]. A session that is **not a member** may post a valid `Join` (it
    /// proves the invite); anything the fold refuses is [`SyncError::NotFound`] for it. Answers the
    /// head after the batch.
    async fn membership_append(
        &self,
        feed: FeedId,
        entries: &[Vec<u8>],
    ) -> Result<MembershipHead, SyncError>;

    /// The membership log after `after_seq` (the whole log when `None`), at most
    /// `CHANGES_PAGE_LIMIT` entries per page. A removed account sees the log up to its own
    /// `Remove` (the page's head is the head at that point).
    async fn membership_get(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> Result<MembershipPage, SyncError>;

    // ---- entries, snapshots, heads ----

    /// Appends a sealed entry (document or keyring) to the feed; answers its cursor and the chain
    /// hash after it. Idempotent on `(feed, doc_id, device_id, device_seq)`: a duplicate answers the
    /// stored cursor and chain hash.
    async fn append(&self, feed: FeedId, entry: &[u8]) -> Result<(u64, [u8; 32]), SyncError>;

    /// The feed after `after`, at most `limit` items, membership entries included; pruned items
    /// are skipped.
    async fn changes(&self, feed: FeedId, after: u64, limit: u32)
    -> Result<ChangesPage, SyncError>;

    /// Stores a sealed snapshot as its document's latest in the feed (Commenter and up). The server
    /// records a snapshot mark. A snapshot that covers less than the stored one is
    /// [`SyncError::Conflict`].
    async fn put_snapshot(&self, feed: FeedId, snapshot: &[u8]) -> Result<(), SyncError>;

    /// A document's latest snapshot in the feed.
    async fn get_snapshot(&self, feed: FeedId, doc: DocId) -> Result<Option<Vec<u8>>, SyncError>;

    /// Every document's latest snapshot in the feed, for bootstrapping.
    async fn snapshots(&self, feed: FeedId) -> Result<SnapshotsPage, SyncError>;

    /// The snapshots published after mark `after` (`GET feeds/{f}/snapshot-marks?after=`): how
    /// Editor-and-up devices learn which snapshots to verify before they acknowledge past `covers`.
    async fn snapshot_marks(&self, feed: FeedId, after: u64) -> Result<MarksPage, SyncError>;

    /// Disputes the latest snapshot of `doc` (the one covering `covers_cursor`): it omits entries it
    /// should contain. The server keeps that document's covered entries (it does not prune them)
    /// until a newer snapshot replaces the disputed one. Editor and up. A document without a
    /// snapshot is [`SyncError::NotFound`]; a dispute of a snapshot that was replaced already is
    /// accepted and changes nothing.
    async fn snapshot_dispute(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> Result<(), SyncError>;

    /// Publishes a signed head (any member), replacing the device's previous one in this feed. It
    /// names the session's account and device, and a cursor the feed has reached.
    async fn put_head(&self, feed: FeedId, head: &[u8]) -> Result<(), SyncError>;

    /// Every device's latest signed head in the feed.
    async fn heads(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, SyncError>;

    // ---- chunks (per feed) ----

    /// Stores a content-addressed chunk (`hash` = BLAKE3 of `bytes`) in the feed's namespace,
    /// counted against the feed's billing account ([`SyncError::Quota`] beyond its quota); storing
    /// it again costs nothing.
    async fn put_chunk(&self, feed: FeedId, hash: ChunkId, bytes: &[u8]) -> Result<(), SyncError>;

    /// The feed's chunk; `None` when the feed holds none under `hash` (whatever other feeds hold).
    async fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<Option<Vec<u8>>, SyncError>;

    /// Deletes the feed's chunk (an Owner of a shared feed; any active device of the account for the
    /// personal feed). A chunk the feed does not hold answers `Ok`.
    async fn delete_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError>;

    /// Copies a chunk of the session account's **personal** namespace into the shared feed's
    /// (`POST feeds/{f}/chunks/{hash}/copy`): the share step moves attachments without uploading
    /// them again. [`SyncError::NotFound`] when the personal namespace holds no such chunk.
    async fn copy_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError>;

    // ---- invites ----

    /// Opens an invite (no session, 30 calls per client per hour): `sig` is
    /// `kb_keys::invite::sign_open` over `"open" ‖ invite_id ‖ server_epoch`. Answers the feed and its
    /// membership log. An unknown, expired, used or revoked invite (or a signature that does not
    /// verify) gets the same [`SyncError::NotFound`]; past the rate limit [`SyncError::Unavailable`].
    async fn invite_open(
        &self,
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<InviteOpened, SyncError>;

    // ---- view links (M13 slice 4, ADR-0051; the rules are in the module documentation) ----

    /// Creates or refreshes a view link's publication (`PUT /v2/links/{id}`, session needed): the
    /// server checks that the upload's publication names `link_id` ([`publication::peek_publication`]),
    /// that the session's account is an Editor or Owner of `put.feed`, and answers the link's new
    /// `pub_seq` (1 at the first upload). The publication is stored as it is: the server holds no key.
    /// A feed that is not the link's feed, a declaration over `LINK_MAX_CHUNKS` or a publication that
    /// does not name `link_id` is [`SyncError::Conflict`], [`SyncError::Protocol`] and
    /// [`SyncError::Protocol`] respectively; a publication over the server's size limit
    /// [`SyncError::Quota`].
    async fn link_put(&self, link_id: LinkId, put: &LinkPut) -> Result<u64, SyncError>;

    /// A link's current publication (`GET /v2/links/{id}`, no session): with `if_none_match` equal to the
    /// stored `pub_seq` the answer is [`LinkFetch::NotModified`] (304), else the publication and its
    /// `pub_seq`. A link that is unknown, expired or revoked is [`SyncError::NotFound`]; past the
    /// read limit [`SyncError::Unavailable`].
    async fn link_get(
        &self,
        link_id: LinkId,
        if_none_match: Option<u64>,
    ) -> Result<LinkFetch, SyncError>;

    /// Revokes a link (`DELETE /v2/links/{id}`, session needed): the row, the publication and the
    /// declared chunks go. The creator (while still a member) or an Owner of the link's feed; another
    /// member is [`SyncError::Forbidden`], a stranger or an unknown link [`SyncError::NotFound`].
    async fn link_delete(&self, link_id: LinkId) -> Result<(), SyncError>;

    /// A sealed chunk the link's publication declared (`GET /v2/links/{id}/chunks/{hash}`, no
    /// session): the feed's chunk under `hash`. A link that is unknown, expired or revoked, a hash the
    /// publication did not declare and a chunk the feed does not hold are one [`SyncError::NotFound`];
    /// past the read limit [`SyncError::Unavailable`].
    async fn link_chunk_get(&self, link_id: LinkId, hash: ChunkId) -> Result<Vec<u8>, SyncError>;

    // ---- pairing mailboxes and recovery blobs (as v1, under /v2) ----

    /// Opens a pairing mailbox (no session) whose message 0 is the opener's `offer`; answers the
    /// random mailbox id and when it expires (Unix ms, server clock).
    async fn pair_create(&self, offer: &[u8]) -> Result<([u8; 6], i64), SyncError>;

    /// The mailbox's messages with `seq >= after`; may hold the request up to `wait_seconds`.
    async fn pair_messages(
        &self,
        id: [u8; 6],
        after: u32,
        wait_seconds: u32,
    ) -> Result<PairingPage, SyncError>;

    /// Posts a message as `role` ([`PAIR_ROLE_OPENER`] or [`PAIR_ROLE_TYPIST`]).
    async fn pair_post(&self, id: [u8; 6], role: u8, bytes: &[u8]) -> Result<(), SyncError>;

    /// Closes the mailbox; closing twice is fine.
    async fn pair_close(&self, id: [u8; 6]) -> Result<(), SyncError>;

    /// Stores the recovery blob under its lookup id (session needed).
    async fn recovery_put(&self, id: [u8; 32], bytes: &[u8]) -> Result<(), SyncError>;

    /// The recovery blob stored under `id` (no session).
    async fn recovery_get(&self, id: [u8; 32]) -> Result<Option<Vec<u8>>, SyncError>;

    /// Deletes the recovery blob (a session of the owning account).
    async fn recovery_delete(&self, id: [u8; 32]) -> Result<(), SyncError>;
}
