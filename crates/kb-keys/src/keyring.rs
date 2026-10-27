//! The keyring (`key-management.md`, "The keyring"; ADR-0019): small signed records
//! wrapping space, document and attachment keys, with a trivial merge rule (union;
//! tombstones win) implemented here, not by the CRDT engine.
//!
//! # Keys
//!
//! - A space key `SK[space][epoch]` is wrapped under the space KEK
//!   `HKDF(ARK, space-kek, info = space id)`.
//! - Document and attachment keys are wrapped under the space key of the record's
//!   `wrap_epoch`. So is the link key `K_link` of a view link (M13 slice 4, ADR-0051): a `Link`
//!   record names the link by its id (`HKDF(K_link, "link-id")[..16]`), and opening it checks that
//!   the key it holds derives that id.
//! - Every wrap names its subject: the AAD subject is `space id || subject bytes`, so
//!   a wrapped key cannot be moved to another space, document, attachment or epoch.
//! - `wrap_epoch` is the generation of the wrapping key: for `Space` records the ARK
//!   generation (0 at creation, +1 per [`rewrap_space_record`]), for `Doc` and
//!   `Attachment` records the epoch of the space key that wraps them.
//!
//! # Encoding
//!
//! ```text
//! subject     u8 1 Space       | u32 epoch
//!             u8 2 Doc         | [16] doc id | u32 epoch
//!             u8 3 Attachment  | [16] attachment id
//!             u8 4 Link        | [16] link id            (M13 slice 4, ADR-0051)
//! record      u8 1 | [16] space id | subject | [72] wrapped key | u32 wrap_epoch |
//!             [8] kcv | [16] created_by | i64 created_at | [64] signature
//! tombstone   u8 2 | [16] space id | subject | [16] purged_by | i64 purged_at |
//!             [64] signature
//! ```
//!
//! Integers are little-endian. The signature (Ed25519, context [`KEYRING_CONTEXT`],
//! by the creating or purging device) covers everything before it, tag byte included.
//! Decoding is strict (canonical).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use kb_crypto::{
    CryptoError, EnvelopeKind, Label, SIGNATURE_LEN, Secret32, Signature, VerifyingKey,
    WRAPPED_KEY_LEN, WrappedKey,
};
use kb_types::{AttachmentId, DeviceId, DocId, LinkId, Rng, SpaceId};

use crate::codec::{DecodeError, Reader};
use crate::device::DeviceKeys;
use crate::link::LinkKey;
use crate::{Ark, KeysError};

/// Signature context of keyring items.
pub const KEYRING_CONTEXT: &str = "keyring";

/// Tag of [`KeySubject::Space`].
const SUBJECT_SPACE: u8 = 1;
/// Tag of [`KeySubject::Doc`].
const SUBJECT_DOC: u8 = 2;
/// Tag of [`KeySubject::Attachment`].
const SUBJECT_ATTACHMENT: u8 = 3;
/// Tag of [`KeySubject::Link`].
const SUBJECT_LINK: u8 = 4;
/// Tag of [`KeyringItem::Record`].
const ITEM_RECORD: u8 = 1;
/// Tag of [`KeyringItem::Tombstone`].
const ITEM_TOMBSTONE: u8 = 2;

/// What a record holds a key for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum KeySubject {
    /// The space key of an epoch.
    Space {
        /// Epoch.
        epoch: u32,
    },
    /// A document key of an epoch.
    Doc {
        /// Document.
        doc_id: DocId,
        /// Epoch.
        epoch: u32,
    },
    /// An attachment key (attachments never rotate).
    Attachment {
        /// Attachment.
        attachment_id: AttachmentId,
    },
    /// The link key `K_link` of a view link (M13 slice 4; view links never rotate). Only the
    /// keyring of the space's feed (or the personal keyring, for a personal space) holds one.
    Link {
        /// The link, `HKDF(K_link, "link-id")[..16]`.
        link_id: LinkId,
    },
}

impl KeySubject {
    /// Canonical bytes (tag byte, then fields; little-endian): 5 bytes for `Space`,
    /// 21 for `Doc`, 17 for `Attachment` and for `Link`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(21);
        self.write(&mut out);
        out
    }

    /// Parses [`Self::encode`] output; anything else is [`KeysError::Corrupted`].
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let mut r = Reader::new(bytes);
        let subject = Self::read(&mut r).map_err(malformed)?;
        r.finish().map_err(malformed)?;
        Ok(subject)
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Space { epoch } => {
                out.push(SUBJECT_SPACE);
                out.extend_from_slice(&epoch.to_le_bytes());
            }
            Self::Doc { doc_id, epoch } => {
                out.push(SUBJECT_DOC);
                out.extend_from_slice(doc_id.as_bytes());
                out.extend_from_slice(&epoch.to_le_bytes());
            }
            Self::Attachment { attachment_id } => {
                out.push(SUBJECT_ATTACHMENT);
                out.extend_from_slice(attachment_id.as_bytes());
            }
            Self::Link { link_id } => {
                out.push(SUBJECT_LINK);
                out.extend_from_slice(link_id.as_bytes());
            }
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match r.u8()? {
            SUBJECT_SPACE => Ok(Self::Space { epoch: r.u32()? }),
            SUBJECT_DOC => Ok(Self::Doc {
                doc_id: DocId::from_bytes(r.array()?),
                epoch: r.u32()?,
            }),
            SUBJECT_ATTACHMENT => Ok(Self::Attachment {
                attachment_id: AttachmentId::from_bytes(r.array()?),
            }),
            SUBJECT_LINK => Ok(Self::Link {
                link_id: LinkId::from_bytes(r.array()?),
            }),
            _ => Err(DecodeError("unknown key subject")),
        }
    }

    /// Name of the subject kind, for errors.
    const fn kind(&self) -> &'static str {
        match self {
            Self::Space { .. } => "space",
            Self::Doc { .. } => "document",
            Self::Attachment { .. } => "attachment",
            Self::Link { .. } => "view link",
        }
    }
}

/// A wrapped key with its provenance, signed by the device that created it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyRecord {
    /// Space.
    pub space_id: SpaceId,
    /// Subject.
    pub subject: KeySubject,
    /// The key, wrapped under the space KEK (space subjects) or the space key
    /// (document and attachment subjects).
    pub wrapped_key: WrappedKey,
    /// Epoch of the wrapping key: the ARK generation for `Space` records, the space key
    /// epoch for `Doc` and `Attachment` records (see the [module documentation](self)).
    pub wrap_epoch: u32,
    /// Key check value of the wrapped key.
    pub kcv: [u8; 8],
    /// Creating device.
    pub created_by: DeviceId,
    /// Unix ms.
    pub created_at: i64,
    /// Signature by the creating device, context [`KEYRING_CONTEXT`].
    pub signature: Signature,
}

/// A purge marker: wins over any record for its subject.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tombstone {
    /// Space.
    pub space_id: SpaceId,
    /// Subject.
    pub subject: KeySubject,
    /// Purging device.
    pub purged_by: DeviceId,
    /// Unix ms.
    pub purged_at: i64,
    /// Signature by the purging device.
    pub signature: Signature,
}

/// A record or a tombstone, as stored and synced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KeyringItem {
    /// A key record.
    Record(KeyRecord),
    /// A tombstone.
    Tombstone(Tombstone),
}

impl KeyringItem {
    /// Canonical bytes (tag byte, then the item; layout in the
    /// [module documentation](self)).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let (mut out, signature) = match self {
            Self::Record(record) => (record.signed_bytes(), record.signature),
            Self::Tombstone(tombstone) => (tombstone.signed_bytes(), tombstone.signature),
        };
        out.extend_from_slice(&signature.to_bytes());
        out
    }

    /// Parses [`Self::encode`] output; anything else is [`KeysError::Corrupted`]. The
    /// signature is not checked here (see [`Self::verify`]).
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let mut r = Reader::new(bytes);
        let item = match r.u8().map_err(malformed)? {
            ITEM_RECORD => {
                let space_id = SpaceId::from_bytes(r.array().map_err(malformed)?);
                let subject = KeySubject::read(&mut r).map_err(malformed)?;
                let wrapped_key =
                    WrappedKey::from_bytes(r.take(WRAPPED_KEY_LEN).map_err(malformed)?)
                        .map_err(|_| corrupted("invalid wrapped key"))?;
                Self::Record(KeyRecord {
                    space_id,
                    subject,
                    wrapped_key,
                    wrap_epoch: r.u32().map_err(malformed)?,
                    kcv: r.array().map_err(malformed)?,
                    created_by: DeviceId::from_bytes(r.array().map_err(malformed)?),
                    created_at: r.i64().map_err(malformed)?,
                    signature: read_signature(&mut r)?,
                })
            }
            ITEM_TOMBSTONE => Self::Tombstone(Tombstone {
                space_id: SpaceId::from_bytes(r.array().map_err(malformed)?),
                subject: KeySubject::read(&mut r).map_err(malformed)?,
                purged_by: DeviceId::from_bytes(r.array().map_err(malformed)?),
                purged_at: r.i64().map_err(malformed)?,
                signature: read_signature(&mut r)?,
            }),
            _ => return Err(corrupted("unknown keyring item kind")),
        };
        r.finish().map_err(malformed)?;
        Ok(item)
    }

    /// The space.
    #[must_use]
    pub fn space_id(&self) -> SpaceId {
        match self {
            Self::Record(r) => r.space_id,
            Self::Tombstone(t) => t.space_id,
        }
    }

    /// The subject.
    #[must_use]
    pub fn subject(&self) -> KeySubject {
        match self {
            Self::Record(r) => r.subject,
            Self::Tombstone(t) => t.subject,
        }
    }

    /// Verifies the signature against the creating device's key (the purging device's
    /// for a tombstone); a mismatch is [`KeysError::Crypto`] (`Authentication`). The
    /// caller resolves `created_by`/`purged_by` to a key through the device log.
    pub fn verify(&self, key: &VerifyingKey) -> Result<(), KeysError> {
        let (signed, signature) = match self {
            Self::Record(record) => (record.signed_bytes(), &record.signature),
            Self::Tombstone(tombstone) => (tombstone.signed_bytes(), &tombstone.signature),
        };
        kb_crypto::verify(key, KEYRING_CONTEXT, &signed, signature)?;
        Ok(())
    }
}

impl KeyRecord {
    /// The encoding without the signature.
    fn signed_bytes(&self) -> Vec<u8> {
        UnsignedRecord {
            space_id: self.space_id,
            subject: self.subject,
            wrapped_key: self.wrapped_key.clone(),
            wrap_epoch: self.wrap_epoch,
            kcv: self.kcv,
            created_by: self.created_by,
            created_at: self.created_at,
        }
        .signed_bytes()
    }
}

impl Tombstone {
    /// The encoding without the signature.
    fn signed_bytes(&self) -> Vec<u8> {
        tombstone_signed_bytes(self.space_id, self.subject, self.purged_by, self.purged_at)
    }
}

/// A record before its signature.
struct UnsignedRecord {
    space_id: SpaceId,
    subject: KeySubject,
    wrapped_key: WrappedKey,
    wrap_epoch: u32,
    kcv: [u8; 8],
    created_by: DeviceId,
    created_at: i64,
}

impl UnsignedRecord {
    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 16 + 21 + WRAPPED_KEY_LEN + 4 + 8 + 16 + 8);
        out.push(ITEM_RECORD);
        out.extend_from_slice(self.space_id.as_bytes());
        self.subject.write(&mut out);
        out.extend_from_slice(&self.wrapped_key.to_bytes());
        out.extend_from_slice(&self.wrap_epoch.to_le_bytes());
        out.extend_from_slice(&self.kcv);
        out.extend_from_slice(self.created_by.as_bytes());
        out.extend_from_slice(&self.created_at.to_le_bytes());
        out
    }

    /// Signs with the creating device's key (`created_by` must name that device).
    fn sign(self, device: &DeviceKeys) -> KeyRecord {
        let signature = kb_crypto::sign(device.signing(), KEYRING_CONTEXT, &self.signed_bytes());
        KeyRecord {
            space_id: self.space_id,
            subject: self.subject,
            wrapped_key: self.wrapped_key,
            wrap_epoch: self.wrap_epoch,
            kcv: self.kcv,
            created_by: self.created_by,
            created_at: self.created_at,
            signature,
        }
    }
}

fn tombstone_signed_bytes(
    space_id: SpaceId,
    subject: KeySubject,
    purged_by: DeviceId,
    purged_at: i64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 16 + 21 + 16 + 8);
    out.push(ITEM_TOMBSTONE);
    out.extend_from_slice(space_id.as_bytes());
    subject.write(&mut out);
    out.extend_from_slice(purged_by.as_bytes());
    out.extend_from_slice(&purged_at.to_le_bytes());
    out
}

fn read_signature(r: &mut Reader<'_>) -> Result<Signature, KeysError> {
    Signature::from_bytes(&r.array::<SIGNATURE_LEN>().map_err(malformed)?)
        .map_err(|_| corrupted("invalid signature"))
}

fn malformed(error: DecodeError) -> KeysError {
    corrupted(error.0)
}

fn corrupted(what: &str) -> KeysError {
    KeysError::Corrupted(format!("malformed keyring item: {what}"))
}

/// Counts for diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KeyringCounts {
    /// Space key records.
    pub spaces: u32,
    /// Document key records.
    pub documents: u32,
    /// Attachment key records.
    pub attachments: u32,
    /// View link key records (M13 slice 4).
    pub links: u32,
    /// Tombstones.
    pub tombstones: u32,
}

/// What a record can be looked up by across spaces: a document or an attachment (a page that
/// moves between spaces, ADR-0038, has `Doc` records in several of them).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Holder {
    Doc(DocId),
    Attachment(AttachmentId),
}

impl Holder {
    fn of(subject: &KeySubject) -> Option<Self> {
        match subject {
            KeySubject::Space { .. } | KeySubject::Link { .. } => None,
            KeySubject::Doc { doc_id, .. } => Some(Self::Doc(*doc_id)),
            KeySubject::Attachment { attachment_id } => Some(Self::Attachment(*attachment_id)),
        }
    }
}

/// The merged keyring of a workspace.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Keyring {
    records: BTreeMap<(SpaceId, KeySubject), KeyRecord>,
    tombstones: BTreeMap<(SpaceId, KeySubject), Tombstone>,
    /// The spaces that hold a live `Doc` record (any epoch) of a document, or an `Attachment`
    /// record of an attachment: a function of `records`, kept in step by [`Self::merge`] and
    /// [`Self::remove_space`].
    holders: BTreeMap<Holder, BTreeSet<SpaceId>>,
}

impl Keyring {
    /// Empty keyring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges one item. Returns whether anything changed.
    ///
    /// For one `(space_id, subject)`:
    ///
    /// - a tombstone wins over every record (the record is dropped, later records for
    ///   the subject are ignored);
    /// - between two records, the one with the higher `wrap_epoch` wins (a `Space`
    ///   record re-wrapped under a rotated ARK replaces the older wrap); for equal
    ///   `wrap_epoch` the earlier `(created_at, created_by)` wins;
    /// - between two tombstones, the earlier `(purged_at, purged_by)` is kept.
    ///
    /// Remaining ties (two different items with all of these equal) go to the smaller
    /// encoding, so the result is a deterministic function of the set of merged items:
    /// merging is idempotent and independent of order. Signatures are not checked here;
    /// verify items from elsewhere with [`KeyringItem::verify`] before merging them.
    pub fn merge(&mut self, item: KeyringItem) -> bool {
        let key = (item.space_id(), item.subject());
        match item {
            KeyringItem::Tombstone(tombstone) => {
                let dropped_record = self.records.remove(&key).is_some();
                if dropped_record {
                    self.unindex(key.0, &key.1);
                }
                let keep_existing = self
                    .tombstones
                    .get(&key)
                    .is_some_and(|existing| !tombstone_wins(&tombstone, existing));
                if !keep_existing {
                    self.tombstones.insert(key, tombstone);
                }
                dropped_record || !keep_existing
            }
            KeyringItem::Record(record) => {
                if self.tombstones.contains_key(&key) {
                    return false;
                }
                let keep_existing = self
                    .records
                    .get(&key)
                    .is_some_and(|existing| !record_wins(&record, existing));
                if !keep_existing {
                    self.records.insert(key, record);
                    self.index(key.0, &key.1);
                }
                !keep_existing
            }
        }
    }

    /// Merges one item of a **shared** feed's keyring, which several accounts write (M13, ADR-0044):
    /// the caller offers the items in feed order and the **first** record for a subject wins, with
    /// `created_at` and `wrap_epoch` ignored, so a hostile member cannot backdate a record to win a
    /// merge. A record for a tombstoned subject, or for a subject that already has one, changes
    /// nothing. A tombstone counts only when `is_owner_at` says the account of the device that
    /// signed it was an Owner at the item's feed cursor (the caller resolves the device through the
    /// membership timeline and the device logs); otherwise it is ignored. A tombstone that counts
    /// drops the subject's record and wins over later records; the first tombstone that counts
    /// stays.
    ///
    /// `Space` subjects never travel in a shared feed (each member keeps the space keys in its own
    /// keyring under its own ARK), so an item for one changes nothing here.
    ///
    /// Returns whether anything changed. Signatures are not checked here (see
    /// [`KeyringItem::verify`]). The personal keyring keeps using [`Self::merge`].
    pub fn merge_ordered(
        &mut self,
        item: KeyringItem,
        is_owner_at: &dyn Fn(&Tombstone) -> bool,
    ) -> bool {
        let key = (item.space_id(), item.subject());
        if matches!(key.1, KeySubject::Space { .. }) {
            return false;
        }
        match item {
            KeyringItem::Tombstone(tombstone) => {
                if self.tombstones.contains_key(&key) || !is_owner_at(&tombstone) {
                    return false;
                }
                if self.records.remove(&key).is_some() {
                    self.unindex(key.0, &key.1);
                }
                self.tombstones.insert(key, tombstone);
                true
            }
            KeyringItem::Record(record) => {
                if self.tombstones.contains_key(&key) || self.records.contains_key(&key) {
                    return false;
                }
                self.records.insert(key, record);
                self.index(key.0, &key.1);
                true
            }
        }
    }

    /// Notes that `space_id` holds a record of the subject (documents and attachments only).
    fn index(&mut self, space_id: SpaceId, subject: &KeySubject) {
        if let Some(holder) = Holder::of(subject) {
            self.holders.entry(holder).or_default().insert(space_id);
        }
    }

    /// Notes that the record of the subject in `space_id` is gone; the space stays listed
    /// while it still holds another epoch of the same document.
    fn unindex(&mut self, space_id: SpaceId, subject: &KeySubject) {
        let Some(holder) = Holder::of(subject) else {
            return;
        };
        let still_holds = match holder {
            Holder::Doc(doc_id) => self
                .records
                .range(
                    (space_id, KeySubject::Doc { doc_id, epoch: 0 })
                        ..=(
                            space_id,
                            KeySubject::Doc {
                                doc_id,
                                epoch: u32::MAX,
                            },
                        ),
                )
                .next()
                .is_some(),
            Holder::Attachment(_) => false,
        };
        if still_holds {
            return;
        }
        if let Some(spaces) = self.holders.get_mut(&holder) {
            spaces.remove(&space_id);
            if spaces.is_empty() {
                self.holders.remove(&holder);
            }
        }
    }

    /// The spaces that hold a live record of the document at any epoch, in space id order. A
    /// page that never moved has one; a page that moved between spaces (ADR-0038) has the
    /// records of the spaces it lived in.
    #[must_use]
    pub fn doc_spaces(&self, doc_id: DocId) -> Vec<SpaceId> {
        self.holders
            .get(&Holder::Doc(doc_id))
            .map(|spaces| spaces.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every live `Doc` record of the document, in every space, by space id then epoch.
    #[must_use]
    pub fn doc_records(&self, doc_id: DocId) -> Vec<&KeyRecord> {
        let mut out = Vec::new();
        for space_id in self.doc_spaces(doc_id) {
            out.extend(
                self.records
                    .range(
                        (space_id, KeySubject::Doc { doc_id, epoch: 0 })
                            ..=(
                                space_id,
                                KeySubject::Doc {
                                    doc_id,
                                    epoch: u32::MAX,
                                },
                            ),
                    )
                    .map(|(_, record)| record),
            );
        }
        out
    }

    /// The spaces that hold a live record of the attachment, in space id order (more than one
    /// after the attachment moved with its pages, ADR-0038).
    #[must_use]
    pub fn attachment_spaces(&self, attachment_id: AttachmentId) -> Vec<SpaceId> {
        self.holders
            .get(&Holder::Attachment(attachment_id))
            .map(|spaces| spaces.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The live record for a subject (none when tombstoned or absent).
    #[must_use]
    pub fn record(&self, space_id: SpaceId, subject: &KeySubject) -> Option<&KeyRecord> {
        self.records.get(&(space_id, *subject))
    }

    /// Whether the subject was purged.
    #[must_use]
    pub fn is_tombstoned(&self, space_id: SpaceId, subject: &KeySubject) -> bool {
        self.tombstones.contains_key(&(space_id, *subject))
    }

    /// Every item, records then tombstones, in key order.
    #[must_use]
    pub fn items(&self) -> Vec<KeyringItem> {
        self.records
            .values()
            .cloned()
            .map(KeyringItem::Record)
            .chain(
                self.tombstones
                    .values()
                    .cloned()
                    .map(KeyringItem::Tombstone),
            )
            .collect()
    }

    /// Records of one space.
    #[must_use]
    pub fn space_records(&self, space_id: SpaceId) -> Vec<&KeyRecord> {
        self.records
            .iter()
            .filter(|((s, _), _)| *s == space_id)
            .map(|(_, r)| r)
            .collect()
    }

    /// Forgets everything about a space (space deletion).
    pub fn remove_space(&mut self, space_id: SpaceId) {
        self.records.retain(|(s, _), _| *s != space_id);
        self.tombstones.retain(|(s, _), _| *s != space_id);
        self.holders.retain(|_, spaces| {
            spaces.remove(&space_id);
            !spaces.is_empty()
        });
    }

    /// Counts.
    #[must_use]
    pub fn counts(&self) -> KeyringCounts {
        let mut counts = KeyringCounts::default();
        for (_, subject) in self.records.keys() {
            let slot = match subject {
                KeySubject::Space { .. } => &mut counts.spaces,
                KeySubject::Doc { .. } => &mut counts.documents,
                KeySubject::Attachment { .. } => &mut counts.attachments,
                KeySubject::Link { .. } => &mut counts.links,
            };
            *slot = slot.saturating_add(1);
        }
        counts.tombstones = u32::try_from(self.tombstones.len()).unwrap_or(u32::MAX);
        counts
    }
}

/// Whether `candidate` displaces `existing` (same space and subject).
fn record_wins(candidate: &KeyRecord, existing: &KeyRecord) -> bool {
    existing
        .wrap_epoch
        .cmp(&candidate.wrap_epoch)
        .then_with(|| candidate.created_at.cmp(&existing.created_at))
        .then_with(|| candidate.created_by.cmp(&existing.created_by))
        .then_with(|| candidate.signed_bytes().cmp(&existing.signed_bytes()))
        .then_with(|| {
            candidate
                .signature
                .to_bytes()
                .cmp(&existing.signature.to_bytes())
        })
        == Ordering::Less
}

/// Whether `candidate` displaces `existing` (same space and subject).
fn tombstone_wins(candidate: &Tombstone, existing: &Tombstone) -> bool {
    candidate
        .purged_at
        .cmp(&existing.purged_at)
        .then_with(|| candidate.purged_by.cmp(&existing.purged_by))
        .then_with(|| {
            candidate
                .signature
                .to_bytes()
                .cmp(&existing.signature.to_bytes())
        })
        == Ordering::Less
}

/// `HKDF(ARK, space-kek, info = space id)`.
#[must_use]
pub fn space_kek(ark: &Ark, space_id: SpaceId) -> Secret32 {
    kb_crypto::derive_key(ark.secret(), Label::SPACE_KEK, space_id.as_bytes())
}

/// The AAD subject of a wrapped key: `space id || subject bytes`. Also the header of a
/// sealed keyring item.
fn wrap_subject(space_id: SpaceId, subject: &KeySubject) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + 21);
    out.extend_from_slice(space_id.as_bytes());
    subject.write(&mut out);
    out
}

/// Who creates a record, and when.
struct Creator<'a> {
    device: &'a DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
}

/// A fresh random key wrapped under `kek` for `subject`, in a signed record.
fn new_record(
    kek: &Secret32,
    space_id: SpaceId,
    subject: KeySubject,
    wrap_epoch: u32,
    creator: &Creator<'_>,
    rng: &dyn Rng,
) -> (Secret32, KeyRecord) {
    let key = Secret32::generate(rng);
    let record = wrap_record(&key, kek, space_id, subject, wrap_epoch, creator, rng);
    (key, record)
}

/// `key` wrapped under `kek` for `subject`, in a signed record.
fn wrap_record(
    key: &Secret32,
    kek: &Secret32,
    space_id: SpaceId,
    subject: KeySubject,
    wrap_epoch: u32,
    creator: &Creator<'_>,
    rng: &dyn Rng,
) -> KeyRecord {
    let wrapped_key = kb_crypto::wrap_key(key, kek, &wrap_subject(space_id, &subject), rng);
    UnsignedRecord {
        space_id,
        subject,
        wrapped_key,
        wrap_epoch,
        kcv: kb_crypto::key_check_value(key),
        created_by: creator.device_id,
        created_at: creator.now_ms,
    }
    .sign(creator.device)
}

/// Unwraps `record` under `kek` after checking its subject kind, then checks the key
/// check value.
fn unwrap_record(
    kek: &Secret32,
    record: &KeyRecord,
    expected: fn(&KeySubject) -> bool,
    kind: &str,
) -> Result<Secret32, KeysError> {
    if !expected(&record.subject) {
        return Err(KeysError::Corrupted(format!(
            "expected a {kind} key record, found a {} key record",
            record.subject.kind()
        )));
    }
    let key = kb_crypto::unwrap_key(
        &record.wrapped_key,
        kek,
        &wrap_subject(record.space_id, &record.subject),
    )?;
    if kb_crypto::key_check_value(&key) != record.kcv {
        return Err(KeysError::Crypto(CryptoError::Authentication));
    }
    Ok(key)
}

/// A fresh space key for `epoch`, wrapped under the space KEK, in a signed record.
///
/// The record's `wrap_epoch` is 0 (the first ARK generation it is wrapped under;
/// [`rewrap_space_record`] counts up from there).
#[must_use]
pub fn new_space_key(
    ark: &Ark,
    space_id: SpaceId,
    epoch: u32,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> (Secret32, KeyRecord) {
    let creator = Creator {
        device,
        device_id,
        now_ms,
    };
    new_record(
        &space_kek(ark, space_id),
        space_id,
        KeySubject::Space { epoch },
        0,
        &creator,
        rng,
    )
}

/// A space key that already exists (opened from a membership box, from a bundle or down the
/// backward key chain) wrapped under the space KEK, in a signed record: what a member's device
/// stores for every key of a shared space it learns (M13, ADR-0044), so its later devices, its
/// backups and its ARK rotations carry the key. `wrap_epoch` is the ARK generation the record is
/// wrapped under ([`rewrap_space_record`] counts up from it).
#[must_use]
// The signature mirrors `new_space_key` with the key supplied.
#[allow(clippy::too_many_arguments)]
pub fn wrap_space_key(
    key: &Secret32,
    ark: &Ark,
    space_id: SpaceId,
    epoch: u32,
    wrap_epoch: u32,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> KeyRecord {
    let creator = Creator {
        device,
        device_id,
        now_ms,
    };
    wrap_record(
        key,
        &space_kek(ark, space_id),
        space_id,
        KeySubject::Space { epoch },
        wrap_epoch,
        &creator,
        rng,
    )
}

/// Unwraps a `Space` record (checks the key check value).
///
/// Another subject kind is [`KeysError::Corrupted`]; a wrong ARK, a record moved to
/// another space or subject, or a key check value that does not match is
/// [`KeysError::Crypto`] (`Authentication`).
pub fn unwrap_space_key(ark: &Ark, record: &KeyRecord) -> Result<Secret32, KeysError> {
    unwrap_record(
        &space_kek(ark, record.space_id),
        record,
        |s| matches!(s, KeySubject::Space { .. }),
        "space",
    )
}

/// A fresh document key, wrapped under the space key of `epoch`.
///
/// The subject is `Doc { doc_id, epoch }` and `wrap_epoch` is `epoch`.
#[must_use]
// The signature is fixed by the core's call sites (Milestone 4 spec).
#[allow(clippy::too_many_arguments)]
pub fn new_doc_key(
    space_key: &Secret32,
    space_id: SpaceId,
    doc_id: DocId,
    epoch: u32,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> (Secret32, KeyRecord) {
    let creator = Creator {
        device,
        device_id,
        now_ms,
    };
    new_record(
        space_key,
        space_id,
        KeySubject::Doc { doc_id, epoch },
        epoch,
        &creator,
        rng,
    )
}

/// Unwraps a `Doc` record with the space key of its `wrap_epoch`.
///
/// Another subject kind is [`KeysError::Corrupted`]; a wrong space key, a record moved
/// to another space, document or epoch, or a key check value that does not match is
/// [`KeysError::Crypto`] (`Authentication`).
pub fn unwrap_doc_key(space_key: &Secret32, record: &KeyRecord) -> Result<Secret32, KeysError> {
    unwrap_record(
        space_key,
        record,
        |s| matches!(s, KeySubject::Doc { .. }),
        "document",
    )
}

/// A fresh attachment key, wrapped under the space key of `epoch`.
///
/// The subject is `Attachment { attachment_id }` and `wrap_epoch` is `epoch`.
#[must_use]
// The signature is fixed by the core's call sites (Milestone 4 spec).
#[allow(clippy::too_many_arguments)]
pub fn new_attachment_key(
    space_key: &Secret32,
    space_id: SpaceId,
    attachment_id: AttachmentId,
    epoch: u32,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> (Secret32, KeyRecord) {
    let creator = Creator {
        device,
        device_id,
        now_ms,
    };
    new_record(
        space_key,
        space_id,
        KeySubject::Attachment { attachment_id },
        epoch,
        &creator,
        rng,
    )
}

/// An existing attachment key wrapped under the space key of `epoch` of another space, in a
/// signed record: what moving an attachment to another space writes (ADR-0038). The key itself
/// is unchanged, so the attachment's chunks (whose AAD names the attachment, not the space)
/// stay valid; the subject is `Attachment { attachment_id }` and `wrap_epoch` is `epoch`.
#[must_use]
// The signature mirrors `new_attachment_key`, which the core's call sites use.
#[allow(clippy::too_many_arguments)]
pub fn wrap_attachment_key(
    attachment_key: &Secret32,
    space_key: &Secret32,
    space_id: SpaceId,
    attachment_id: AttachmentId,
    epoch: u32,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> KeyRecord {
    let creator = Creator {
        device,
        device_id,
        now_ms,
    };
    wrap_record(
        attachment_key,
        space_key,
        space_id,
        KeySubject::Attachment { attachment_id },
        epoch,
        &creator,
        rng,
    )
}

/// Unwraps an `Attachment` record (with the space key of its `wrap_epoch`); errors as
/// for [`unwrap_doc_key`].
pub fn unwrap_attachment_key(
    space_key: &Secret32,
    record: &KeyRecord,
) -> Result<Secret32, KeysError> {
    unwrap_record(
        space_key,
        record,
        |s| matches!(s, KeySubject::Attachment { .. }),
        "attachment",
    )
}

/// A view link's key wrapped under the space key of `epoch`, in a signed record (M13 slice 4,
/// ADR-0051): the `Link` record every member device of the space can open to refresh the link or to
/// show its URL again. The subject is `Link { link_id }` with the id the key derives, and
/// `wrap_epoch` is `epoch`.
#[must_use]
// The signature mirrors `wrap_attachment_key`, which the core's call sites use.
#[allow(clippy::too_many_arguments)]
pub fn wrap_link_key(
    link_key: &LinkKey,
    space_key: &Secret32,
    space_id: SpaceId,
    epoch: u32,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> KeyRecord {
    let creator = Creator {
        device,
        device_id,
        now_ms,
    };
    wrap_record(
        link_key.secret(),
        space_key,
        space_id,
        KeySubject::Link {
            link_id: link_key.link_id(),
        },
        epoch,
        &creator,
        rng,
    )
}

/// Unwraps a `Link` record (with the space key of its `wrap_epoch`) and checks that the key it
/// holds derives the record's link id; errors as for [`unwrap_doc_key`], and a key that does not
/// derive its own subject is [`KeysError::Corrupted`] (a record whose subject was swapped for
/// another link's fails the wrap's AAD first, with `Authentication`).
pub fn unwrap_link_key(space_key: &Secret32, record: &KeyRecord) -> Result<LinkKey, KeysError> {
    let KeySubject::Link { link_id } = record.subject else {
        return Err(KeysError::Corrupted(format!(
            "expected a view link key record, found a {} key record",
            record.subject.kind()
        )));
    };
    let key = unwrap_record(
        space_key,
        record,
        |s| matches!(s, KeySubject::Link { .. }),
        "view link",
    )?;
    let key = LinkKey::from_secret(key);
    if key.link_id() != link_id {
        return Err(KeysError::Corrupted(
            "a view link record holds a key that does not derive its link id".to_owned(),
        ));
    }
    Ok(key)
}

/// Re-wraps a `Space` record under a new ARK (same space key, same epoch), as a new
/// signed record by this device.
///
/// The new record keeps `space_id`, `subject` and the key check value, has
/// `wrap_epoch = record.wrap_epoch + 1` (so [`Keyring::merge`] prefers it over the old
/// wrap in any order), `created_by = device_id` and `created_at = now_ms`. Errors are
/// those of [`unwrap_space_key`] with `old_ark`; a `wrap_epoch` that cannot grow is
/// [`KeysError::Corrupted`].
pub fn rewrap_space_record(
    record: &KeyRecord,
    old_ark: &Ark,
    new_ark: &Ark,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
    rng: &dyn Rng,
) -> Result<KeyRecord, KeysError> {
    let key = unwrap_space_key(old_ark, record)?;
    let wrap_epoch = record
        .wrap_epoch
        .checked_add(1)
        .ok_or_else(|| KeysError::Corrupted("the record's wrap epoch is exhausted".to_owned()))?;
    let wrapped_key = kb_crypto::wrap_key(
        &key,
        &space_kek(new_ark, record.space_id),
        &wrap_subject(record.space_id, &record.subject),
        rng,
    );
    Ok(UnsignedRecord {
        space_id: record.space_id,
        subject: record.subject,
        wrapped_key,
        wrap_epoch,
        kcv: kb_crypto::key_check_value(&key),
        created_by: device_id,
        created_at: now_ms,
    }
    .sign(device))
}

/// A signed tombstone for a subject.
#[must_use]
pub fn tombstone(
    space_id: SpaceId,
    subject: KeySubject,
    device: &DeviceKeys,
    device_id: DeviceId,
    now_ms: i64,
) -> Tombstone {
    let signature = kb_crypto::sign(
        device.signing(),
        KEYRING_CONTEXT,
        &tombstone_signed_bytes(space_id, subject, device_id, now_ms),
    );
    Tombstone {
        space_id,
        subject,
        purged_by: device_id,
        purged_at: now_ms,
        signature,
    }
}

/// The transport key of keyring items: `HKDF(ARK, keyring)`.
fn keyring_key(ark: &Ark) -> Secret32 {
    kb_crypto::derive_key(ark.secret(), Label::KEYRING, b"")
}

/// Seals an item for transport (envelope kind `Keyring`, key `HKDF(ARK, keyring)`,
/// header = space id || subject bytes). Used by sync (M6).
///
/// The envelope carries no signature of its own: the item inside is signed by its
/// device.
#[must_use]
pub fn seal_item(item: &KeyringItem, ark: &Ark, rng: &dyn Rng) -> Vec<u8> {
    kb_crypto::seal_envelope(
        EnvelopeKind::Keyring,
        &wrap_subject(item.space_id(), &item.subject()),
        &keyring_key(ark),
        &item.encode(),
        None,
        rng,
    )
}

/// Reverses [`seal_item`].
///
/// A wrong ARK or tampering is [`KeysError::Crypto`]; an envelope of another kind, one
/// carrying a signature, an item that does not decode or an item that does not match
/// the envelope header is [`KeysError::Corrupted`]. The item's own signature is not
/// checked here (see [`KeyringItem::verify`]).
pub fn open_item(bytes: &[u8], ark: &Ark) -> Result<KeyringItem, KeysError> {
    let (envelope, plaintext) = kb_crypto::open_envelope(bytes, &keyring_key(ark), None)?;
    if envelope.kind != EnvelopeKind::Keyring || envelope.signature.is_some() {
        return Err(KeysError::Corrupted(
            "not a keyring item envelope".to_owned(),
        ));
    }
    let item = KeyringItem::decode(&plaintext)?;
    if envelope.header != wrap_subject(item.space_id(), &item.subject()) {
        return Err(KeysError::Corrupted(
            "the keyring envelope header does not match its item".to_owned(),
        ));
    }
    Ok(item)
}

#[cfg(test)]
mod tests;
