//! The objects every sync protocol version shares (ADR-0021, ADR-0045): the encrypted bodies of
//! entries, keyring entries and snapshots, the pages of changes and snapshots, the authentication
//! message, the chain hash, the pairing mailbox messages, and the sealing and verification helpers
//! the feed objects of [`v2`] are built from. **Protocol v1 is retired** (ADR-0045: v2 replaces it
//! without a server migration): the headers, the remote, the reference remote and the conformance
//! suite are in [`v2`], which keeps its path.
//!
//! Pure and wasm32-capable: no I/O, no async runtime. The only asynchrony is the `async fn`s of
//! [`v2::RemoteStore`], which `kb-net` drives on tokio and the fleet simulator drives through
//! `MemoryRemote`'s synchronous methods.
//!
//! # Formats (integers little-endian, ids 16 raw bytes, blobs `u32 len || bytes`)
//!
//! ```text
//! entry body       u8 engine | u16 format_version | u16 min_reader_version |
//!                  u8 doc_kind | u32 n | blob update ^n | blob version_vector
//! keyring body     u32 n | blob KeyringItem::encode() ^n
//! snapshot body    u8 engine | u16 format_version | u16 min_reader_version |
//!                  u8 doc_kind | blob snapshot | blob version_vector
//! changes page     u8 1 | [16] server_epoch | u32 count |
//!                  (u64 cursor | blob item)^count | [32] chain_hash_after | u8 more
//! snapshots page   u8 1 | [16] server_epoch | u32 count | blob snapshot ^count
//! auth message     [32] nonce | [16] server_epoch, signed with context "auth"
//! ```
//!
//! The headers (entry 77 bytes, keyring 61, snapshot 77, head 113 and its 64-byte signature) are
//! those of [`v2`]. Entries, keyring entries and snapshots are envelope v1
//! (`kb_crypto::envelope`) of kind `Entry`, `Keyring` and `Snapshot`: the header is the envelope
//! header (in the clear, authenticated as AAD), the body is bucket-padded
//! (`kb_crypto::pad_to_bucket`) and encrypted, and the device signs the whole envelope (context
//! `envelope/<kind>`). Opening verifies the signature before anything is decrypted. Malformed bytes
//! are [`SyncError::Encoding`]; a failed signature or AEAD tag is [`SyncError::Authentication`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use kb_crypto::{
    Envelope, EnvelopeKind, SIGNATURE_LEN, Secret32, Signature, SigningKey, VerifyingKey,
};
use kb_keys::KeyringItem;
use kb_types::sync::{CHANGES_PAGE_LIMIT, MAX_WIRE_BYTES};
use kb_types::{AccountId, DeviceId, DocId, Rng};
use zeroize::Zeroizing;

use crate::codec::{Reader, put_blob, put_count};

mod codec;
pub mod v2;

#[cfg(test)]
mod tests;

/// Signature context of authentication messages.
pub const AUTH_CONTEXT: &str = "auth";
/// Version byte of [`ChangesPage`] and [`SnapshotsPage`].
pub const PAGE_VERSION: u8 = 1;

/// Everything that can go wrong at the protocol level. Messages never carry key
/// material or content; a server error message is passed on as it came.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    /// Bytes that do not decode as the expected object.
    #[error("malformed sync data: {0}")]
    Encoding(String),
    /// A signature or an AEAD tag failed.
    #[error("sync data failed authentication")]
    Authentication,
    /// The other side broke the protocol (bad epoch, gap, oversize, ...).
    #[error("protocol violation: {0}")]
    Protocol(String),
    /// The account quota is exhausted or the object is too large for the server.
    #[error("quota exceeded")]
    Quota,
    /// Unknown account, device, document or object.
    #[error("not found")]
    NotFound,
    /// The device is not authenticated or not active in the account's log.
    #[error("unauthorized")]
    Unauthorized,
    /// The device is active in the log but the server refuses it because its last
    /// session is older than the server's idle-device limit (ADR-0022 §6); another
    /// device has to re-admit it.
    #[error("this device was idle too long")]
    Idle,
    /// The server cannot be reached or answered a server-side failure.
    #[error("server unavailable: {0}")]
    Unavailable(String),
    /// A request conflicts with the server's state (an account that already exists, a
    /// log that does not continue the stored one, ...).
    #[error("conflict: {0}")]
    Conflict(String),
    /// A content append reached a feed that waits for a `Rekey` (409 `rekey_needed`, v2): the next
    /// writer's device mints it (M13, ADR-0044).
    #[error("the space needs a new key before it accepts changes")]
    RekeyNeeded,
    /// The entry's key epoch is below the feed's (409 `stale_epoch`, v2): the client re-seals its
    /// outbox under the current epoch.
    #[error("the entry was sealed under an old key epoch")]
    StaleEpoch,
    /// The feed was closed by an Owner (409 `feed_closed`, v2): it accepts no more writes.
    #[error("the space was closed")]
    Closed,
    /// The session's account may not do this: a role that is too low, a removed account reading past
    /// its `Remove` (403 `forbidden`), or the share signup limits used up (403 `signup_limit`, v2).
    #[error("forbidden")]
    Forbidden,
}

/// Encrypted body of a document entry: the pending updates of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryBody {
    /// CRDT engine id of the updates.
    pub engine: u8,
    /// Document format version of the writer.
    pub format_version: u16,
    /// Oldest format version that can read the document after these updates.
    pub min_reader_version: u16,
    /// Structure, page or account document.
    pub doc_kind: u8,
    /// Engine updates, kept as separate blobs in their original order.
    pub updates: Vec<Vec<u8>>,
    /// The writer's version vector after the updates.
    pub version_vector: Vec<u8>,
}

/// Encrypted body of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBody {
    /// CRDT engine id.
    pub engine: u8,
    /// Document format version of the writer.
    pub format_version: u16,
    /// Oldest format version that can read the snapshot.
    pub min_reader_version: u16,
    /// Structure, page or account document.
    pub doc_kind: u8,
    /// The engine's full snapshot.
    pub snapshot: Vec<u8>,
    /// The version vector the snapshot reflects.
    pub version_vector: Vec<u8>,
}

/// One page of a feed, as `changes` answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesPage {
    /// The server's storage epoch.
    pub server_epoch: [u8; 16],
    /// `(cursor, bytes)` in cursor order.
    pub changes: Vec<(u64, Vec<u8>)>,
    /// The chain hash after the last change of this page (after the caller's `after`
    /// when the page is empty).
    pub chain_hash_after: [u8; 32],
    /// More changes follow.
    pub more: bool,
}

/// Every document's latest snapshot of a feed, for bootstrapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotsPage {
    /// The server's storage epoch.
    pub server_epoch: [u8; 16],
    /// Sealed snapshots, one per document.
    pub snapshots: Vec<Vec<u8>>,
}

/// [`PairingMessage::role`] of the joiner (the new device; it posts message 0, the
/// offer).
pub const PAIRING_ROLE_JOINER: u8 = 0;
/// [`PairingMessage::role`] of the sponsor (the existing device).
pub const PAIRING_ROLE_SPONSOR: u8 = 1;

/// One message of a pairing mailbox (ADR-0022 §1): opaque bytes the server relays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingMessage {
    /// Position in the mailbox, from 0 (the joiner's offer) without gaps.
    pub seq: u32,
    /// Who posted it: [`PAIRING_ROLE_JOINER`] or [`PAIRING_ROLE_SPONSOR`].
    pub role: u8,
    /// The message (the clear offer and answer, then `Pairing` envelopes).
    pub bytes: Vec<u8>,
}

/// What `pair_messages` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPage {
    /// The messages with `seq >= after`, in order.
    pub messages: Vec<PairingMessage>,
    /// When the mailbox expires (Unix ms, server clock).
    pub expires_at: i64,
    /// `pair_close` was called: nothing more can be posted.
    pub closed: bool,
}

// ----- shared helpers --------------------------------------------------------------------

/// Maps a `kb-crypto` failure to the protocol error: tag or signature failures are
/// [`SyncError::Authentication`], everything else is malformed input.
fn crypto_error(error: kb_crypto::CryptoError) -> SyncError {
    match error {
        kb_crypto::CryptoError::Authentication => SyncError::Authentication,
        kb_crypto::CryptoError::Encoding(message) => SyncError::Encoding(message),
        other => SyncError::Encoding(other.to_string()),
    }
}

/// Decodes an envelope and checks its kind.
fn decode_envelope(bytes: &[u8], kind: EnvelopeKind) -> Result<Envelope, SyncError> {
    let envelope = Envelope::decode(bytes).map_err(crypto_error)?;
    if envelope.kind != kind {
        return Err(SyncError::Encoding(format!(
            "expected an {} envelope, found {}",
            kind.context(),
            envelope.kind.context()
        )));
    }
    Ok(envelope)
}

/// Checks the envelope's signature (mandatory) over every byte before it.
fn check_signature(
    bytes: &[u8],
    envelope: &Envelope,
    verifier: &VerifyingKey,
) -> Result<(), SyncError> {
    let signature = envelope
        .signature
        .as_ref()
        .ok_or(SyncError::Authentication)?;
    // The envelope encoding is canonical, so the signed bytes are the input minus the
    // trailing signature.
    let signed_len = bytes
        .len()
        .checked_sub(SIGNATURE_LEN)
        .ok_or(SyncError::Authentication)?;
    let signed = bytes.get(..signed_len).ok_or(SyncError::Authentication)?;
    kb_crypto::verify(verifier, envelope.kind.context(), signed, signature)
        .map_err(|_| SyncError::Authentication)
}

/// Verifies the signature, then decrypts and unpads; the plaintext stays zeroised.
fn open_body(
    bytes: &[u8],
    envelope: &Envelope,
    key: &Secret32,
    verifier: &VerifyingKey,
) -> Result<Zeroizing<Vec<u8>>, SyncError> {
    check_signature(bytes, envelope, verifier)?;
    let padded = kb_crypto::aead_open(key, &envelope.nonce, &envelope.aad(), &envelope.ciphertext)
        .map_err(crypto_error)?;
    let content = kb_crypto::unpad(&padded).map_err(crypto_error)?;
    Ok(Zeroizing::new(content.to_vec()))
}

/// Pads the body and seals it under `key`, signed by `signer`. An unrepresentable
/// body gives an empty result, which every decoder refuses (closed failure).
fn seal_body(
    kind: EnvelopeKind,
    header: &[u8],
    body: Option<Zeroizing<Vec<u8>>>,
    key: &Secret32,
    signer: &SigningKey,
    rng: &dyn Rng,
) -> Vec<u8> {
    let Some(body) = body else {
        return Vec::new();
    };
    let padded = Zeroizing::new(kb_crypto::pad_to_bucket(&body, rng));
    if padded.is_empty() {
        return Vec::new();
    }
    kb_crypto::seal_envelope(kind, header, key, &padded, Some(signer), rng)
}

fn read_account(r: &mut Reader<'_>) -> Result<AccountId, SyncError> {
    r.array("account id").map(AccountId::from_bytes)
}

fn read_doc(r: &mut Reader<'_>) -> Result<DocId, SyncError> {
    r.array("document id").map(DocId::from_bytes)
}

fn read_device(r: &mut Reader<'_>) -> Result<DeviceId, SyncError> {
    r.array("device id").map(DeviceId::from_bytes)
}

/// The common prefix of entry and snapshot bodies.
fn put_body_prefix(
    out: &mut Vec<u8>,
    engine: u8,
    format_version: u16,
    min_reader_version: u16,
    doc_kind: u8,
) {
    out.push(engine);
    out.extend_from_slice(&format_version.to_le_bytes());
    out.extend_from_slice(&min_reader_version.to_le_bytes());
    out.push(doc_kind);
}

/// `(engine, format_version, min_reader_version, doc_kind)`.
fn read_body_prefix(r: &mut Reader<'_>) -> Result<(u8, u16, u16, u8), SyncError> {
    Ok((
        r.u8("engine")?,
        r.u16("format version")?,
        r.u16("minimum reader version")?,
        r.u8("document kind")?,
    ))
}

// ----- document entries ---------------------------------------------------------------

fn encode_entry_body(body: &EntryBody) -> Option<Zeroizing<Vec<u8>>> {
    let len = body
        .updates
        .iter()
        .fold(6 + 4 + 4 + body.version_vector.len(), |acc, u| {
            acc.saturating_add(4).saturating_add(u.len())
        });
    let mut out = Zeroizing::new(Vec::with_capacity(len));
    put_body_prefix(
        &mut out,
        body.engine,
        body.format_version,
        body.min_reader_version,
        body.doc_kind,
    );
    put_count(&mut out, body.updates.len())?;
    for update in &body.updates {
        put_blob(&mut out, update)?;
    }
    put_blob(&mut out, &body.version_vector)?;
    Some(out)
}

fn decode_entry_body(bytes: &[u8]) -> Result<EntryBody, SyncError> {
    let mut r = Reader::new(bytes, "entry body");
    let (engine, format_version, min_reader_version, doc_kind) = read_body_prefix(&mut r)?;
    let count = r.count("update count", 4)?;
    let mut updates = Vec::with_capacity(count);
    for _ in 0..count {
        updates.push(r.blob("update")?.to_vec());
    }
    let version_vector = r.blob("version vector")?.to_vec();
    r.finish()?;
    Ok(EntryBody {
        engine,
        format_version,
        min_reader_version,
        doc_kind,
        updates,
        version_vector,
    })
}

// ----- keyring entries ----------------------------------------------------------------

/// The key keyring entries are sealed under: `HKDF(ARK, "keyring")`, the same key
/// `kb_keys::keyring::seal_item` uses.
#[must_use]
pub fn keyring_key(ark: &kb_keys::Ark) -> Secret32 {
    kb_crypto::derive_key(ark.secret(), kb_crypto::Label::KEYRING, b"")
}

fn encode_keyring_body(items: &[KeyringItem]) -> Option<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::new());
    put_count(&mut out, items.len())?;
    for item in items {
        put_blob(&mut out, &item.encode())?;
    }
    Some(out)
}

fn decode_keyring_body(bytes: &[u8]) -> Result<Vec<KeyringItem>, SyncError> {
    let mut r = Reader::new(bytes, "keyring body");
    let count = r.count("item count", 4)?;
    let mut items = Vec::with_capacity(count);
    for index in 0..count {
        let encoded = r.blob("keyring item")?;
        let item = KeyringItem::decode(encoded).map_err(|_| {
            SyncError::Encoding(format!("keyring body: item {index} does not decode"))
        })?;
        items.push(item);
    }
    r.finish()?;
    Ok(items)
}

// ----- snapshots ----------------------------------------------------------------------

fn encode_snapshot_body(body: &SnapshotBody) -> Option<Zeroizing<Vec<u8>>> {
    let len = (6 + 4 + 4usize)
        .saturating_add(body.snapshot.len())
        .saturating_add(body.version_vector.len());
    let mut out = Zeroizing::new(Vec::with_capacity(len));
    put_body_prefix(
        &mut out,
        body.engine,
        body.format_version,
        body.min_reader_version,
        body.doc_kind,
    );
    put_blob(&mut out, &body.snapshot)?;
    put_blob(&mut out, &body.version_vector)?;
    Some(out)
}

fn decode_snapshot_body(bytes: &[u8]) -> Result<SnapshotBody, SyncError> {
    let mut r = Reader::new(bytes, "snapshot body");
    let (engine, format_version, min_reader_version, doc_kind) = read_body_prefix(&mut r)?;
    let snapshot = r.blob("snapshot")?.to_vec();
    let version_vector = r.blob("version vector")?.to_vec();
    r.finish()?;
    Ok(SnapshotBody {
        engine,
        format_version,
        min_reader_version,
        doc_kind,
        snapshot,
        version_vector,
    })
}

// ----- heads ----------------------------------------------------------------------------

// ----- feed helpers ---------------------------------------------------------------------

/// `chain_hash_0 = BLAKE3(server_epoch)`.
#[must_use]
pub fn chain_genesis(server_epoch: &[u8; 16]) -> [u8; 32] {
    kb_crypto::hash(server_epoch)
}

/// `chain_hash_n = BLAKE3(chain_hash_{n-1} || bytes_n)`.
#[must_use]
pub fn chain_next(prev: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
    kb_crypto::chain(prev, bytes)
}

/// Refuses an item larger than [`MAX_WIRE_BYTES`].
pub(crate) fn check_item_size(len: usize, what: &str) -> Result<(), SyncError> {
    if u64::try_from(len).map_or(true, |len| len > MAX_WIRE_BYTES) {
        Err(SyncError::Protocol(format!(
            "{what} of {len} bytes exceeds {MAX_WIRE_BYTES}"
        )))
    } else {
        Ok(())
    }
}

impl ChangesPage {
    /// Binary framing of ADR-0021 §2: `u8 1 || [16] server_epoch || u32 count ||
    /// (u64 cursor || u32 len || bytes)^count || [32] chain_hash_after || u8 more`.
    /// A page that cannot be represented (an item beyond `u32::MAX` bytes) encodes as
    /// an empty vector, which [`Self::decode`] refuses.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let len = self
            .changes
            .iter()
            .fold(1 + 16 + 4 + 32 + 1, |acc: usize, (_, bytes)| {
                acc.saturating_add(12).saturating_add(bytes.len())
            });
        let mut out = Vec::with_capacity(len);
        out.push(PAGE_VERSION);
        out.extend_from_slice(&self.server_epoch);
        let framed = (|| {
            put_count(&mut out, self.changes.len())?;
            for (cursor, bytes) in &self.changes {
                out.extend_from_slice(&cursor.to_le_bytes());
                put_blob(&mut out, bytes)?;
            }
            Some(())
        })();
        if framed.is_none() {
            return Vec::new();
        }
        out.extend_from_slice(&self.chain_hash_after);
        out.push(u8::from(self.more));
        out
    }

    /// Inverse of [`ChangesPage::encode`]. A wrong version, truncation, trailing
    /// bytes or a `more` byte other than 0 or 1 are [`SyncError::Encoding`]; more than
    /// [`CHANGES_PAGE_LIMIT`] changes, an item beyond [`MAX_WIRE_BYTES`] or cursors that
    /// do not strictly increase are [`SyncError::Protocol`].
    pub fn decode(bytes: &[u8]) -> Result<Self, SyncError> {
        let mut r = Reader::new(bytes, "changes page");
        read_page_version(&mut r)?;
        let server_epoch = r.array("server epoch")?;
        let count = r.u32("change count")?;
        if count > CHANGES_PAGE_LIMIT {
            return Err(SyncError::Protocol(format!(
                "a page of {count} changes exceeds {CHANGES_PAGE_LIMIT}"
            )));
        }
        let count = r.fit_count(count, "change count", 8 + 4)?;
        let mut changes: Vec<(u64, Vec<u8>)> = Vec::with_capacity(count);
        for _ in 0..count {
            let cursor = r.u64("cursor")?;
            let item = r.blob("change")?;
            check_item_size(item.len(), "a change")?;
            if changes
                .last()
                .is_some_and(|(previous, _)| cursor <= *previous)
            {
                return Err(SyncError::Protocol(
                    "the page's cursors do not strictly increase".to_owned(),
                ));
            }
            changes.push((cursor, item.to_vec()));
        }
        let chain_hash_after = r.array("chain hash")?;
        let more = match r.u8("more flag")? {
            0 => false,
            1 => true,
            other => {
                return Err(SyncError::Encoding(format!(
                    "changes page: more flag {other} is not 0 or 1"
                )));
            }
        };
        r.finish()?;
        Ok(Self {
            server_epoch,
            changes,
            chain_hash_after,
            more,
        })
    }
}

fn read_page_version(r: &mut Reader<'_>) -> Result<(), SyncError> {
    let version = r.u8("version")?;
    if version == PAGE_VERSION {
        Ok(())
    } else {
        Err(SyncError::Encoding(format!(
            "unsupported page version {version}"
        )))
    }
}

impl SnapshotsPage {
    /// Same framing as the changes page without cursors, chain hash and `more`:
    /// `u8 1 || [16] server_epoch || u32 count || (u32 len || bytes)^count`. An
    /// unrepresentable page encodes as an empty vector.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let len = self.snapshots.iter().fold(1 + 16 + 4, |acc: usize, bytes| {
            acc.saturating_add(4).saturating_add(bytes.len())
        });
        let mut out = Vec::with_capacity(len);
        out.push(PAGE_VERSION);
        out.extend_from_slice(&self.server_epoch);
        let framed = (|| {
            put_count(&mut out, self.snapshots.len())?;
            for bytes in &self.snapshots {
                put_blob(&mut out, bytes)?;
            }
            Some(())
        })();
        if framed.is_none() {
            return Vec::new();
        }
        out
    }

    /// Inverse of [`SnapshotsPage::encode`]. A wrong version, truncation or trailing
    /// bytes are [`SyncError::Encoding`]; an item beyond [`MAX_WIRE_BYTES`] is
    /// [`SyncError::Protocol`]. The page is not limited in count (it holds every
    /// document's latest snapshot).
    pub fn decode(bytes: &[u8]) -> Result<Self, SyncError> {
        let mut r = Reader::new(bytes, "snapshots page");
        read_page_version(&mut r)?;
        let server_epoch = r.array("server epoch")?;
        let count = r.count("snapshot count", 4)?;
        let mut snapshots = Vec::with_capacity(count);
        for _ in 0..count {
            let item = r.blob("snapshot")?;
            check_item_size(item.len(), "a snapshot")?;
            snapshots.push(item.to_vec());
        }
        r.finish()?;
        Ok(Self {
            server_epoch,
            snapshots,
        })
    }
}

// ----- authentication -------------------------------------------------------------------

/// The bytes a device signs to open a session: `nonce || server_epoch`, context `auth`.
#[must_use]
pub fn auth_message(nonce: &[u8; 32], server_epoch: &[u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    out.extend_from_slice(nonce);
    out.extend_from_slice(server_epoch);
    out
}

/// Signs an authentication message (context [`AUTH_CONTEXT`]).
#[must_use]
pub fn sign_auth(message: &[u8], signer: &SigningKey) -> Signature {
    kb_crypto::sign(signer, AUTH_CONTEXT, message)
}

/// Verifies an authentication signature; a mismatch is [`SyncError::Authentication`].
pub fn verify_auth(
    message: &[u8],
    signature: &Signature,
    verifier: &VerifyingKey,
) -> Result<(), SyncError> {
    kb_crypto::verify(verifier, AUTH_CONTEXT, message, signature)
        .map_err(|_| SyncError::Authentication)
}
