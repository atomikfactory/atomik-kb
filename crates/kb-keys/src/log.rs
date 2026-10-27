//! The device log: an append-only, hash-chained, signed log per account
//! (`key-management.md`, "Account and device identity"). The account id is the hash
//! of the Genesis entry.
//!
//! # Encoding (entry version 1)
//!
//! Little-endian integers, `u32` length prefixes for byte strings, one tag byte per
//! body variant and per [`Signer`]:
//!
//! ```text
//! u16 version = 1 | u64 seq | [32] prev_hash | u8 body tag | body | [64] signature
//!
//! tag 1 Genesis          [16] device_id | [32] signing key | [32] agreement key |
//!                        [32] recovery key | u32 len + encrypted_name | i64 at
//! tag 2 DeviceAdded      [16] device_id | [32] signing key | [32] agreement key |
//!                        u32 len + encrypted_name | signer | u8 via_kit (0 or 1) | i64 at
//! tag 3 DeviceRevoked    [16] device_id | [16] revoked_by | i64 at
//! tag 4 RecoveryRotated  [32] recovery key | [16] by | i64 at
//! tag 5 RootRotated      u32 generation | u32 count (at most 1024) |
//!                        count x ([16] device_id | u32 len + sealed box) | [16] by | i64 at
//!
//! signer: u8 1 (Device) followed by [16] device id | u8 2 (Recovery)
//! ```
//!
//! The signature (Ed25519, context [`LOG_CONTEXT`]) covers everything before it
//! ([`LogEntry::signed_bytes`]); the entry hash ([`LogEntry::hash`]) is BLAKE3 of the
//! whole encoding, signature included, and is what the next entry's `prev_hash` and
//! the account id are made of. Decoding is strict, so every accepted byte string is
//! the canonical encoding of its entry.
//!
//! # Verification rules
//!
//! [`DeviceLog::verify`] replays the log from the start and fails on the first entry
//! that breaks a rule:
//!
//! - entry 0 is a Genesis and no other entry is ([`LogError::Genesis`]);
//! - `seq` counts from 0 and `prev_hash` is the hash of the previous entry (zeros for
//!   Genesis) ([`LogError::Chain`]);
//! - the signer named in the body must be able to sign at that point: the Genesis
//!   device for the Genesis, an active device for `DeviceAdded { added_by: Device }`,
//!   `DeviceRevoked`, `RecoveryRotated` and `RootRotated`, the current recovery key
//!   for `DeviceAdded { added_by: Recovery }` ([`LogError::InactiveSigner`]), and the
//!   signature must verify under that key ([`LogError::Signature`]);
//! - `via_kit` is `true` exactly when `added_by` is [`Signer::Recovery`]
//!   ([`LogError::Encoding`]);
//! - a device is added once ever, revoked devices included ([`LogError::DuplicateDevice`]);
//! - only an active device can be revoked ([`LogError::NotActive`]), and not by itself
//!   ([`LogError::InactiveSigner`]: a device cannot sign its own revocation);
//! - `RecoveryRotated` replaces the recovery key that later kit-signed entries must
//!   verify under;
//! - `RootRotated` names the generation that follows the current one (0 at Genesis,
//!   [`LogError::Encoding`] otherwise), seals at most [`MAX_ROOT_BOXES`] boxes
//!   ([`LogError::Encoding`]), and seals exactly one box to every active device
//!   other than the signer: a box for a device that is not active or for the signer
//!   is [`LogError::NotActive`], a device boxed twice [`LogError::DuplicateDevice`],
//!   an active device left out [`LogError::IncompleteRotation`]. The boxes
//!   themselves are opaque here (`rotation::open_root_box` opens one).
//!
//! Timestamps are informational and not ordered.

use kb_crypto::{
    AgreementPublic, CryptoError, NONCE_LEN, Nonce24, SIGNATURE_LEN, Signature, SigningKey,
    TAG_LEN, VerifyingKey,
};
use kb_types::{AccountId, DeviceId, Rng};

use crate::codec::{DecodeError, Reader, put_bytes};
use crate::device::{DeviceKeys, DevicePublicKeys};
use crate::{Ark, KeysError};

/// Entry encoding version.
pub const LOG_ENTRY_VERSION: u16 = 1;
/// Signature context of log entries.
pub const LOG_CONTEXT: &str = "device-log";

/// Body tag of [`LogEntryBody::Genesis`].
const TAG_GENESIS: u8 = 1;
/// Body tag of [`LogEntryBody::DeviceAdded`].
const TAG_DEVICE_ADDED: u8 = 2;
/// Body tag of [`LogEntryBody::DeviceRevoked`].
const TAG_DEVICE_REVOKED: u8 = 3;
/// Body tag of [`LogEntryBody::RecoveryRotated`].
const TAG_RECOVERY_ROTATED: u8 = 4;
/// Body tag of [`LogEntryBody::RootRotated`] (Milestone 7).
const TAG_ROOT_ROTATED: u8 = 5;
/// Most boxes a `RootRotated` entry carries (one per remaining active device).
pub const MAX_ROOT_BOXES: usize = 1024;
/// Tag of [`Signer::Device`].
const SIGNER_DEVICE: u8 = 1;
/// Tag of [`Signer::Recovery`].
const SIGNER_RECOVERY: u8 = 2;

/// Who signed an entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signer {
    /// An active device of the account.
    Device(DeviceId),
    /// The recovery signing key (derived from the recovery kit).
    Recovery,
}

/// What an entry records.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LogEntryBody {
    /// The first entry: the first device and the recovery public key.
    Genesis {
        /// First device.
        device_id: DeviceId,
        /// Its public keys.
        keys: DevicePublicKeys,
        /// Recovery signing public key.
        recovery_key: VerifyingKey,
        /// Device name encrypted under HKDF(ARK, `device-name`).
        encrypted_name: Vec<u8>,
        /// Unix ms.
        at: i64,
    },
    /// A device joined.
    DeviceAdded {
        /// New device.
        device_id: DeviceId,
        /// Its public keys.
        keys: DevicePublicKeys,
        /// Encrypted name.
        encrypted_name: Vec<u8>,
        /// Who added it.
        added_by: Signer,
        /// `true` when the entry was signed by the recovery key.
        via_kit: bool,
        /// Unix ms.
        at: i64,
    },
    /// A device was revoked.
    DeviceRevoked {
        /// Revoked device.
        device_id: DeviceId,
        /// Active device that revoked it.
        revoked_by: DeviceId,
        /// Unix ms.
        at: i64,
    },
    /// The recovery kit was re-issued.
    RecoveryRotated {
        /// New recovery signing public key.
        recovery_key: VerifyingKey,
        /// Active device that rotated it.
        by: DeviceId,
        /// Unix ms.
        at: i64,
    },
    /// The root key rotated (ADR-0022): the new key sealed to every other active
    /// device's agreement key (`rotation::seal_root_for`).
    RootRotated {
        /// The new generation (`LogState::root_generation` + 1).
        generation: u32,
        /// One sealed box per remaining active device (the rotating device excluded).
        boxes: Vec<(DeviceId, Vec<u8>)>,
        /// Active device that rotated.
        by: DeviceId,
        /// Unix ms.
        at: i64,
    },
}

/// One signed, chained entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LogEntry {
    /// [`LOG_ENTRY_VERSION`].
    pub version: u16,
    /// 0 for Genesis, then +1.
    pub seq: u64,
    /// Hash of the previous entry (zeros for Genesis).
    pub prev_hash: [u8; 32],
    /// Payload.
    pub body: LogEntryBody,
    /// Signature over [`Self::signed_bytes`] with [`LOG_CONTEXT`].
    pub signature: Signature,
}

impl LogEntry {
    /// Canonical binary encoding (little-endian, length-prefixed strings and byte
    /// strings); what is stored, hashed and (minus the signature) signed. The layout is
    /// in the [module documentation](self).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.signed_bytes();
        out.extend_from_slice(&self.signature.to_bytes());
        out
    }

    /// Parses [`Self::encode`] output; unknown versions and malformed bytes are
    /// [`LogError::Encoding`].
    pub fn decode(bytes: &[u8]) -> Result<Self, LogError> {
        let mut r = Reader::new(bytes);
        let version = r.u16().map_err(malformed)?;
        if version != LOG_ENTRY_VERSION {
            return Err(LogError::Encoding(format!(
                "unsupported entry version {version}"
            )));
        }
        let seq = r.u64().map_err(malformed)?;
        let prev_hash = r.array().map_err(malformed)?;
        let body = read_body(&mut r)?;
        let signature = Signature::from_bytes(&r.array::<SIGNATURE_LEN>().map_err(malformed)?)
            .map_err(|_| LogError::Encoding("invalid signature".to_owned()))?;
        r.finish().map_err(malformed)?;
        Ok(Self {
            version,
            seq,
            prev_hash,
            body,
            signature,
        })
    }

    /// Everything but the signature, in encoding order.
    #[must_use]
    pub fn signed_bytes(&self) -> Vec<u8> {
        unsigned_bytes(self.version, self.seq, &self.prev_hash, &self.body)
    }

    /// BLAKE3 of [`Self::encode`].
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        kb_crypto::hash(&self.encode())
    }

    /// A version-1 entry signed by `key`.
    fn signed(seq: u64, prev_hash: [u8; 32], body: LogEntryBody, key: &SigningKey) -> Self {
        let signature = kb_crypto::sign(
            key,
            LOG_CONTEXT,
            &unsigned_bytes(LOG_ENTRY_VERSION, seq, &prev_hash, &body),
        );
        Self {
            version: LOG_ENTRY_VERSION,
            seq,
            prev_hash,
            body,
            signature,
        }
    }
}

/// The encoding of an entry without its signature.
fn unsigned_bytes(version: u16, seq: u64, prev_hash: &[u8; 32], body: &LogEntryBody) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(prev_hash);
    match body {
        LogEntryBody::Genesis {
            device_id,
            keys,
            recovery_key,
            encrypted_name,
            at,
        } => {
            out.push(TAG_GENESIS);
            out.extend_from_slice(device_id.as_bytes());
            put_keys(&mut out, keys);
            out.extend_from_slice(&recovery_key.to_bytes());
            put_bytes(&mut out, encrypted_name);
            out.extend_from_slice(&at.to_le_bytes());
        }
        LogEntryBody::DeviceAdded {
            device_id,
            keys,
            encrypted_name,
            added_by,
            via_kit,
            at,
        } => {
            out.push(TAG_DEVICE_ADDED);
            out.extend_from_slice(device_id.as_bytes());
            put_keys(&mut out, keys);
            put_bytes(&mut out, encrypted_name);
            match added_by {
                Signer::Device(by) => {
                    out.push(SIGNER_DEVICE);
                    out.extend_from_slice(by.as_bytes());
                }
                Signer::Recovery => out.push(SIGNER_RECOVERY),
            }
            out.push(u8::from(*via_kit));
            out.extend_from_slice(&at.to_le_bytes());
        }
        LogEntryBody::DeviceRevoked {
            device_id,
            revoked_by,
            at,
        } => {
            out.push(TAG_DEVICE_REVOKED);
            out.extend_from_slice(device_id.as_bytes());
            out.extend_from_slice(revoked_by.as_bytes());
            out.extend_from_slice(&at.to_le_bytes());
        }
        LogEntryBody::RecoveryRotated {
            recovery_key,
            by,
            at,
        } => {
            out.push(TAG_RECOVERY_ROTATED);
            out.extend_from_slice(&recovery_key.to_bytes());
            out.extend_from_slice(by.as_bytes());
            out.extend_from_slice(&at.to_le_bytes());
        }
        LogEntryBody::RootRotated {
            generation,
            boxes,
            by,
            at,
        } => {
            out.push(TAG_ROOT_ROTATED);
            out.extend_from_slice(&generation.to_le_bytes());
            let count = u32::try_from(boxes.len()).unwrap_or(u32::MAX);
            out.extend_from_slice(&count.to_le_bytes());
            for (device_id, sealed) in boxes.iter().take(count as usize) {
                out.extend_from_slice(device_id.as_bytes());
                put_bytes(&mut out, sealed);
            }
            out.extend_from_slice(by.as_bytes());
            out.extend_from_slice(&at.to_le_bytes());
        }
    }
    out
}

/// Signing key, then agreement key.
fn put_keys(out: &mut Vec<u8>, keys: &DevicePublicKeys) {
    out.extend_from_slice(&keys.signing.to_bytes());
    out.extend_from_slice(&keys.agreement.to_bytes());
}

/// The body: tag byte, then the variant's fields.
fn read_body(r: &mut Reader<'_>) -> Result<LogEntryBody, LogError> {
    let body = match r.u8().map_err(malformed)? {
        TAG_GENESIS => LogEntryBody::Genesis {
            device_id: read_device_id(r)?,
            keys: read_keys(r)?,
            recovery_key: read_verifying_key(r)?,
            encrypted_name: r.bytes().map_err(malformed)?.to_vec(),
            at: r.i64().map_err(malformed)?,
        },
        TAG_DEVICE_ADDED => LogEntryBody::DeviceAdded {
            device_id: read_device_id(r)?,
            keys: read_keys(r)?,
            encrypted_name: r.bytes().map_err(malformed)?.to_vec(),
            added_by: match r.u8().map_err(malformed)? {
                SIGNER_DEVICE => Signer::Device(read_device_id(r)?),
                SIGNER_RECOVERY => Signer::Recovery,
                other => return Err(LogError::Encoding(format!("unknown signer kind {other}"))),
            },
            via_kit: r.bool().map_err(malformed)?,
            at: r.i64().map_err(malformed)?,
        },
        TAG_DEVICE_REVOKED => LogEntryBody::DeviceRevoked {
            device_id: read_device_id(r)?,
            revoked_by: read_device_id(r)?,
            at: r.i64().map_err(malformed)?,
        },
        TAG_RECOVERY_ROTATED => LogEntryBody::RecoveryRotated {
            recovery_key: read_verifying_key(r)?,
            by: read_device_id(r)?,
            at: r.i64().map_err(malformed)?,
        },
        TAG_ROOT_ROTATED => {
            let generation = r.u32().map_err(malformed)?;
            let count = r.u32().map_err(malformed)?;
            if usize::try_from(count).unwrap_or(usize::MAX) > MAX_ROOT_BOXES {
                return Err(LogError::Encoding(format!(
                    "a root rotation names {count} devices"
                )));
            }
            let mut boxes = Vec::new();
            for _ in 0..count {
                let device_id = read_device_id(r)?;
                let sealed = r.bytes().map_err(malformed)?.to_vec();
                boxes.push((device_id, sealed));
            }
            LogEntryBody::RootRotated {
                generation,
                boxes,
                by: read_device_id(r)?,
                at: r.i64().map_err(malformed)?,
            }
        }
        other => return Err(LogError::Encoding(format!("unknown entry kind {other}"))),
    };
    Ok(body)
}

fn read_device_id(r: &mut Reader<'_>) -> Result<DeviceId, LogError> {
    Ok(DeviceId::from_bytes(r.array().map_err(malformed)?))
}

fn read_verifying_key(r: &mut Reader<'_>) -> Result<VerifyingKey, LogError> {
    VerifyingKey::from_bytes(&r.array::<32>().map_err(malformed)?)
        .map_err(|_| LogError::Encoding("invalid signing public key".to_owned()))
}

fn read_keys(r: &mut Reader<'_>) -> Result<DevicePublicKeys, LogError> {
    let signing = read_verifying_key(r)?;
    let agreement = AgreementPublic::from_bytes(&r.array::<32>().map_err(malformed)?)
        .map_err(|_| LogError::Encoding("invalid agreement public key".to_owned()))?;
    Ok(DevicePublicKeys { signing, agreement })
}

fn malformed(error: DecodeError) -> LogError {
    LogError::Encoding(error.0.to_owned())
}

/// Why a log does not verify. Messages carry sequence numbers only.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LogError {
    /// No entries.
    #[error("the device log is empty")]
    Empty,
    /// The first entry is not a Genesis, or a Genesis appears later.
    #[error("entry {seq} is not where a Genesis belongs")]
    Genesis {
        /// Offending entry.
        seq: u64,
    },
    /// `seq` or `prev_hash` does not continue the chain.
    #[error("entry {seq} breaks the chain")]
    Chain {
        /// Offending entry.
        seq: u64,
    },
    /// Signature does not verify.
    #[error("entry {seq} has a bad signature")]
    Signature {
        /// Offending entry.
        seq: u64,
    },
    /// Signed by a device the log does not know, or by a revoked device.
    #[error("entry {seq} is signed by a device that is not active")]
    InactiveSigner {
        /// Offending entry.
        seq: u64,
    },
    /// Adding a device that is already in the log, or a root rotation that seals
    /// two boxes to one device.
    #[error("entry {seq} adds a device that already exists or names one twice")]
    DuplicateDevice {
        /// Offending entry.
        seq: u64,
    },
    /// Revoking a device that is unknown or already revoked, or sealing a root key
    /// box to such a device or to the rotating device itself.
    #[error("entry {seq} names a device that is not active")]
    NotActive {
        /// Offending entry.
        seq: u64,
    },
    /// A root rotation that leaves out an active device (other than the signer).
    #[error("entry {seq} does not seal the root key to every active device")]
    IncompleteRotation {
        /// Offending entry.
        seq: u64,
    },
    /// Malformed bytes or an unknown version.
    #[error("malformed device log entry: {0}")]
    Encoding(String),
}

/// A device as the verified log describes it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceRecord {
    /// Device.
    pub device_id: DeviceId,
    /// Public keys.
    pub keys: DevicePublicKeys,
    /// Encrypted name (see [`decrypt_name`]).
    pub encrypted_name: Vec<u8>,
    /// When it joined, Unix ms.
    pub added_at: i64,
    /// Added through the recovery kit.
    pub added_via_kit: bool,
    /// Who added it (`Signer::Recovery` for a kit join; the first device counts as
    /// added by itself).
    pub added_by: Signer,
    /// The device that revoked it, when it was.
    pub revoked_by: Option<DeviceId>,
    /// When it was revoked, if it was.
    pub revoked_at: Option<i64>,
}

/// The state a verified log describes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LogState {
    /// BLAKE3(Genesis)[..16].
    pub account_id: AccountId,
    /// Current recovery signing public key.
    pub recovery_key: VerifyingKey,
    /// Every device ever added, in log order.
    pub devices: Vec<DeviceRecord>,
    /// Root key generation: 0 at genesis, +1 per `RootRotated`.
    pub root_generation: u32,
}

impl LogState {
    /// When a device was revoked, if it was.
    #[must_use]
    pub fn revoked_at(&self, device_id: DeviceId) -> Option<i64> {
        self.devices
            .iter()
            .find(|d| d.device_id == device_id)
            .and_then(|d| d.revoked_at)
    }

    /// The device if it is known and not revoked.
    #[must_use]
    pub fn active(&self, device_id: DeviceId) -> Option<&DeviceRecord> {
        self.devices
            .iter()
            .find(|d| d.device_id == device_id && d.revoked_at.is_none())
    }
}

/// Who signs a new entry. Its `Debug` output is safe: the key types redact themselves.
#[derive(Clone, Copy, Debug)]
pub enum LogSigner<'a> {
    /// This device.
    Device(&'a DeviceKeys, DeviceId),
    /// The recovery signing key.
    Recovery(&'a SigningKey),
}

/// The log of one account, verified on construction and on every append.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DeviceLog {
    entries: Vec<LogEntry>,
}

impl DeviceLog {
    /// Wraps stored entries, verifying the whole chain (an empty list is
    /// [`LogError::Empty`]).
    pub fn from_entries(entries: Vec<LogEntry>) -> Result<Self, LogError> {
        let log = Self { entries };
        log.verify()?;
        Ok(log)
    }

    /// Creates a log with its Genesis entry, signed by the first device.
    pub fn genesis(
        device_id: DeviceId,
        device: &DeviceKeys,
        recovery_key: VerifyingKey,
        encrypted_name: Vec<u8>,
        at: i64,
    ) -> Result<Self, LogError> {
        let body = LogEntryBody::Genesis {
            device_id,
            keys: device.public(),
            recovery_key,
            encrypted_name,
            at,
        };
        let entry = LogEntry::signed(0, [0; 32], body, device.signing());
        Replay::default().apply(&entry)?;
        Ok(Self {
            entries: vec![entry],
        })
    }

    /// The entries in order.
    #[must_use]
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// Hash of the last entry.
    #[must_use]
    pub fn head_hash(&self) -> Option<[u8; 32]> {
        self.entries.last().map(LogEntry::hash)
    }

    /// The public keys of a device the log ever added, revoked or not. A log is verified on
    /// construction and on every append, so this reads the entries without checking them again
    /// (the membership fold resolves entry signers with it: an entry written before a device was
    /// revoked stays valid).
    #[must_use]
    pub fn device_keys(&self, device_id: DeviceId) -> Option<DevicePublicKeys> {
        self.entries.iter().find_map(|entry| match &entry.body {
            LogEntryBody::Genesis {
                device_id: id,
                keys,
                ..
            }
            | LogEntryBody::DeviceAdded {
                device_id: id,
                keys,
                ..
            } if *id == device_id => Some(*keys),
            _ => None,
        })
    }

    /// The hash of entry `seq` ([`LogEntry::hash`]); `None` when the log has no such entry.
    #[must_use]
    pub fn entry_hash(&self, seq: u64) -> Option<[u8; 32]> {
        let index = usize::try_from(seq).ok()?;
        self.entries.get(index).map(LogEntry::hash)
    }

    /// The devices that were active right after entry `seq` (added and not revoked by then), in
    /// the order they were added; `None` when the log has no entry `seq`. A `Rekey` boxes the
    /// space key for exactly these devices as of each member's head (ADR-0044).
    #[must_use]
    pub fn active_devices_at(&self, seq: u64) -> Option<Vec<(DeviceId, DevicePublicKeys)>> {
        let end = usize::try_from(seq).ok()?.checked_add(1)?;
        let entries = self.entries.get(..end)?;
        let mut active: Vec<(DeviceId, DevicePublicKeys)> = Vec::new();
        for entry in entries {
            match &entry.body {
                LogEntryBody::Genesis {
                    device_id, keys, ..
                }
                | LogEntryBody::DeviceAdded {
                    device_id, keys, ..
                } => active.push((*device_id, *keys)),
                LogEntryBody::DeviceRevoked { device_id, .. } => {
                    active.retain(|(id, _)| id != device_id);
                }
                LogEntryBody::RecoveryRotated { .. } | LogEntryBody::RootRotated { .. } => {}
            }
        }
        Some(active)
    }

    /// Verifies the chain and the signatures and returns the resulting state (rules
    /// in the [module documentation](self)).
    pub fn verify(&self) -> Result<LogState, LogError> {
        self.replay()?.state.ok_or(LogError::Empty)
    }

    /// Appends `DeviceAdded`, signed by an active device or the recovery key.
    ///
    /// [`LogSigner::Device`] records `added_by: Device(id), via_kit: false`;
    /// [`LogSigner::Recovery`] records `added_by: Recovery, via_kit: true`. The new
    /// entry must verify (the signing device active and its keys the ones in the log,
    /// or the recovery key the current one; the device not added before); otherwise
    /// the error is returned and the log is unchanged.
    pub fn add_device(
        &mut self,
        device_id: DeviceId,
        keys: DevicePublicKeys,
        encrypted_name: Vec<u8>,
        signer: LogSigner<'_>,
        at: i64,
    ) -> Result<&LogEntry, LogError> {
        let (added_by, via_kit, key) = match signer {
            LogSigner::Device(device, by) => (Signer::Device(by), false, device.signing()),
            LogSigner::Recovery(key) => (Signer::Recovery, true, key),
        };
        let body = LogEntryBody::DeviceAdded {
            device_id,
            keys,
            encrypted_name,
            added_by,
            via_kit,
            at,
        };
        self.push_signed(body, key)
    }

    /// Appends `DeviceRevoked`, signed by an active device (not the revoked one).
    /// A rule violation is returned and leaves the log unchanged.
    pub fn revoke_device(
        &mut self,
        device_id: DeviceId,
        by: (&DeviceKeys, DeviceId),
        at: i64,
    ) -> Result<&LogEntry, LogError> {
        let (device, revoked_by) = by;
        let body = LogEntryBody::DeviceRevoked {
            device_id,
            revoked_by,
            at,
        };
        self.push_signed(body, device.signing())
    }

    /// Appends `RecoveryRotated`, signed by an active device. Later kit-signed entries
    /// must verify under `recovery_key`. A rule violation is returned and leaves the
    /// log unchanged.
    pub fn rotate_recovery(
        &mut self,
        recovery_key: VerifyingKey,
        by: (&DeviceKeys, DeviceId),
        at: i64,
    ) -> Result<&LogEntry, LogError> {
        let (device, by) = by;
        let body = LogEntryBody::RecoveryRotated {
            recovery_key,
            by,
            at,
        };
        self.push_signed(body, device.signing())
    }

    /// Appends `RootRotated` (ADR-0022), signed by an active device: `boxes` holds
    /// the new root key sealed to every other active device, each exactly once
    /// (`rotation::seal_root_for`). A rule violation is returned and leaves the log
    /// unchanged.
    pub fn rotate_root(
        &mut self,
        generation: u32,
        boxes: Vec<(DeviceId, Vec<u8>)>,
        by: (&DeviceKeys, DeviceId),
        at: i64,
    ) -> Result<&LogEntry, LogError> {
        let (device, by) = by;
        let body = LogEntryBody::RootRotated {
            generation,
            boxes,
            by,
            at,
        };
        self.push_signed(body, device.signing())
    }

    /// Appends an entry received from elsewhere (sync), verifying it first. On an
    /// empty log the entry must be a Genesis. A rejected entry leaves the log
    /// unchanged.
    pub fn append(&mut self, entry: LogEntry) -> Result<(), LogError> {
        let mut replay = self.replay()?;
        replay.apply(&entry)?;
        self.entries.push(entry);
        Ok(())
    }

    /// Replays every entry.
    fn replay(&self) -> Result<Replay, LogError> {
        let mut replay = Replay::default();
        for entry in &self.entries {
            replay.apply(entry)?;
        }
        Ok(replay)
    }

    /// Signs `body` as the next entry, checks it against the current state and appends it.
    fn push_signed(&mut self, body: LogEntryBody, key: &SigningKey) -> Result<&LogEntry, LogError> {
        let mut replay = self.replay()?;
        if replay.state.is_none() {
            return Err(LogError::Empty);
        }
        let entry = LogEntry::signed(replay.next_seq, replay.head, body, key);
        replay.apply(&entry)?;
        self.entries.push(entry);
        self.entries.last().ok_or(LogError::Empty)
    }
}

/// The verifier's state while it walks the log.
#[derive(Default)]
struct Replay {
    /// `None` until the Genesis was applied.
    state: Option<LogState>,
    /// Hash of the last applied entry (zeros before Genesis).
    head: [u8; 32],
    /// `seq` the next entry must carry.
    next_seq: u64,
}

impl Replay {
    /// Checks `entry` against the state and applies it; on error nothing changes.
    fn apply(&mut self, entry: &LogEntry) -> Result<(), LogError> {
        let seq = entry.seq;
        if entry.version != LOG_ENTRY_VERSION {
            return Err(LogError::Encoding(format!(
                "entry {seq} has unsupported version {}",
                entry.version
            )));
        }
        let is_genesis = matches!(entry.body, LogEntryBody::Genesis { .. });
        if is_genesis == self.state.is_some() {
            return Err(LogError::Genesis { seq });
        }
        if seq != self.next_seq || entry.prev_hash != self.head {
            return Err(LogError::Chain { seq });
        }
        let next_seq = seq.checked_add(1).ok_or(LogError::Chain { seq })?;

        let signed = entry.signed_bytes();
        let check_signature = |key: &VerifyingKey| {
            kb_crypto::verify(key, LOG_CONTEXT, &signed, &entry.signature)
                .map_err(|_| LogError::Signature { seq })
        };

        match (&mut self.state, &entry.body) {
            (
                None,
                LogEntryBody::Genesis {
                    device_id,
                    keys,
                    recovery_key,
                    encrypted_name,
                    at,
                },
            ) => {
                check_signature(&keys.signing)?;
                self.state = Some(LogState {
                    account_id: account_id_of(entry),
                    recovery_key: *recovery_key,
                    devices: vec![DeviceRecord {
                        device_id: *device_id,
                        keys: *keys,
                        encrypted_name: encrypted_name.clone(),
                        added_at: *at,
                        added_via_kit: false,
                        added_by: Signer::Device(*device_id),
                        revoked_by: None,
                        revoked_at: None,
                    }],
                    root_generation: 0,
                });
            }
            (
                Some(state),
                LogEntryBody::DeviceAdded {
                    device_id,
                    keys,
                    encrypted_name,
                    added_by,
                    via_kit,
                    at,
                },
            ) => {
                if *via_kit != matches!(added_by, Signer::Recovery) {
                    return Err(LogError::Encoding(format!(
                        "entry {seq}: via_kit does not match the signer"
                    )));
                }
                let signer_key = match added_by {
                    Signer::Device(by) => active_signing_key(state, *by, seq)?,
                    Signer::Recovery => state.recovery_key,
                };
                check_signature(&signer_key)?;
                if state.devices.iter().any(|d| d.device_id == *device_id) {
                    return Err(LogError::DuplicateDevice { seq });
                }
                state.devices.push(DeviceRecord {
                    device_id: *device_id,
                    keys: *keys,
                    encrypted_name: encrypted_name.clone(),
                    added_at: *at,
                    added_via_kit: *via_kit,
                    added_by: *added_by,
                    revoked_by: None,
                    revoked_at: None,
                });
            }
            (
                Some(state),
                LogEntryBody::DeviceRevoked {
                    device_id,
                    revoked_by,
                    at,
                },
            ) => {
                check_signature(&active_signing_key(state, *revoked_by, seq)?)?;
                if device_id == revoked_by {
                    return Err(LogError::InactiveSigner { seq });
                }
                let target = state
                    .devices
                    .iter_mut()
                    .find(|d| d.device_id == *device_id && d.revoked_at.is_none())
                    .ok_or(LogError::NotActive { seq })?;
                target.revoked_at = Some(*at);
                target.revoked_by = Some(*revoked_by);
            }
            (
                Some(state),
                LogEntryBody::RootRotated {
                    generation,
                    boxes,
                    by,
                    ..
                },
            ) => {
                check_signature(&active_signing_key(state, *by, seq)?)?;
                if state.root_generation.checked_add(1) != Some(*generation) {
                    return Err(LogError::Encoding(format!(
                        "entry {seq}: root generation {generation} does not follow {}",
                        state.root_generation
                    )));
                }
                if boxes.len() > MAX_ROOT_BOXES {
                    return Err(LogError::Encoding(format!(
                        "entry {seq}: a root rotation seals at most {MAX_ROOT_BOXES} boxes"
                    )));
                }
                let mut seen = Vec::with_capacity(boxes.len());
                for (device_id, _) in boxes {
                    if device_id == by || state.active(*device_id).is_none() {
                        return Err(LogError::NotActive { seq });
                    }
                    if seen.contains(device_id) {
                        return Err(LogError::DuplicateDevice { seq });
                    }
                    seen.push(*device_id);
                }
                let others = state
                    .devices
                    .iter()
                    .filter(|d| d.revoked_at.is_none() && d.device_id != *by)
                    .count();
                if seen.len() != others {
                    return Err(LogError::IncompleteRotation { seq });
                }
                state.root_generation = *generation;
            }
            (
                Some(state),
                LogEntryBody::RecoveryRotated {
                    recovery_key, by, ..
                },
            ) => {
                check_signature(&active_signing_key(state, *by, seq)?)?;
                state.recovery_key = *recovery_key;
            }
            // Excluded by the Genesis placement check above.
            (None, _) | (Some(_), LogEntryBody::Genesis { .. }) => {
                return Err(LogError::Genesis { seq });
            }
        }

        self.head = entry.hash();
        self.next_seq = next_seq;
        Ok(())
    }
}

/// The signing key of an active device, or [`LogError::InactiveSigner`].
fn active_signing_key(
    state: &LogState,
    device_id: DeviceId,
    seq: u64,
) -> Result<VerifyingKey, LogError> {
    state
        .active(device_id)
        .map(|d| d.keys.signing)
        .ok_or(LogError::InactiveSigner { seq })
}

/// `BLAKE3(encoded Genesis)[..16]`.
#[must_use]
pub fn account_id_of(genesis: &LogEntry) -> AccountId {
    let hash = genesis.hash();
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash[..16]);
    AccountId::from_bytes(id)
}

/// The key of device names: `HKDF(ARK, device-name, info = device id bytes)`.
fn name_key(ark: &Ark, device_id: DeviceId) -> kb_crypto::Secret32 {
    kb_crypto::derive_key(
        ark.secret(),
        kb_crypto::Label::DEVICE_NAME,
        device_id.as_bytes(),
    )
}

/// Encrypts a device name under HKDF(ARK, `device-name`) with the device id as AAD.
///
/// XChaCha20-Poly1305 under `HKDF(ARK, device-name, info = device id bytes)` with a
/// random nonce and AAD = the device id bytes; the output is `nonce || ciphertext ||
/// tag` (name length + 40 bytes). Names are not padded, so their length is visible
/// to whoever sees the log.
#[must_use]
pub fn encrypt_name(ark: &Ark, device_id: DeviceId, name: &str, rng: &dyn Rng) -> Vec<u8> {
    let nonce = Nonce24::random(rng);
    let ciphertext = kb_crypto::aead_seal(
        &name_key(ark, device_id),
        &nonce,
        device_id.as_bytes(),
        name.as_bytes(),
    );
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce.as_bytes());
    out.extend_from_slice(&ciphertext);
    out
}

/// Reverses [`encrypt_name`]. Input shorter than a nonce and a tag is
/// [`KeysError::Crypto`] (`Encoding`); a wrong ARK or device id, or tampering, is
/// [`KeysError::Crypto`] (`Authentication`); a plaintext that is not UTF-8 is
/// [`KeysError::Corrupted`].
pub fn decrypt_name(ark: &Ark, device_id: DeviceId, bytes: &[u8]) -> Result<String, KeysError> {
    let Some((nonce, ciphertext)) = bytes.split_first_chunk::<NONCE_LEN>() else {
        return Err(too_short_name());
    };
    if ciphertext.len() < TAG_LEN {
        return Err(too_short_name());
    }
    let plaintext = kb_crypto::aead_open(
        &name_key(ark, device_id),
        &Nonce24::from_bytes(*nonce),
        device_id.as_bytes(),
        ciphertext,
    )?;
    String::from_utf8(plaintext.to_vec())
        .map_err(|_| KeysError::Corrupted("the device name is not UTF-8".to_owned()))
}

fn too_short_name() -> KeysError {
    KeysError::Crypto(CryptoError::Encoding(
        "the encrypted device name is too short".to_owned(),
    ))
}

#[cfg(test)]
mod tests;
