//! The canonical encodings of membership entries: the clear header (layout in the
//! [parent module](super)) and the sealed private part.
//!
//! ```text
//! header   u8 2 | [16] feed | u64 seq | [32] prev | [16] account | [16] device | u8 kind |
//!          fields | i64 at | u32 meta_epoch
//!
//! fields by kind (little-endian, strict):
//!   Create        u32 epoch | [32] sk_commitment | boxes | heads
//!   Invite        [16] invite_id | [32] invite_pub | u8 role | i64 expires_at |
//!                 u8 bound (0, or 1 then [16] account) | u8 verified |
//!                 u8 bundle (0, or 1 then u32 len + enc_bundle)
//!   InviteRevoke  [16] invite_id
//!   Join          [16] invite_id | [64] proof | [16] account | u64 log_seq | [32] log_hash
//!   RoleSet       [16] target | u8 role
//!   Remove        [16] target | rekey
//!   Leave         (none)
//!   Rekey         rekey
//!   Grant         [16] member | u32 epoch | boxes
//!   Profile       (none)
//!   Settings      (none)
//!   Close         (none)
//!
//! rekey    u32 new_epoch | [32] sk_commitment | [72] prev_wrapped | boxes | heads
//! boxes    u32 count (at most 1024) | count x ([16] device | u32 len + box)
//! heads    u32 count (at most 50) | count x ([16] account | u64 seq | [32] hash)
//! role     1 Viewer, 2 Commenter, 3 Editor, 4 Owner
//!
//! private  (sealed under MK; the kinds without private fields seal an empty plaintext)
//!   Create    u8 1 | u32 len + space name | u32 trash_retention_days | u8 editors_may_link |
//!             u32 len + profile name
//!   Join      u8 2 | u32 len + profile name
//!   Profile   u8 3 | u32 len + display name
//!   Settings  u8 4 | u32 trash_retention_days | u8 editors_may_link
//! ```
//!
//! Profile names are 1 to `PROFILE_NAME_MAX_CHARS` characters. Decoding is strict, so every
//! accepted byte string is the canonical encoding of its value.

use kb_crypto::{SIGNATURE_LEN, Signature, VerifyingKey, WRAPPED_KEY_LEN, WrappedKey};
use kb_types::sharing::{
    INVITE_BUNDLE_LOG_MAX_BYTES, MAX_BOXES_PER_ENTRY, MAX_MEMBERS_PER_FEED, PROFILE_NAME_MAX_CHARS,
    SpaceSettings,
};
use kb_types::{AccountId, DeviceId, FeedId, InviteId, MemberRole};

use super::{
    ChainHead, EntryBody, EntryKind, KeyBox, MEMBERSHIP_VERSION, MemberHead, MembershipHeader,
    Private, RekeyFields,
};
use crate::KeysError;
use crate::codec::{DecodeError, Reader, put_bytes};

/// Tag of [`Private::Create`].
const PRIVATE_CREATE: u8 = 1;
/// Tag of [`Private::Join`].
const PRIVATE_JOIN: u8 = 2;
/// Tag of [`Private::Profile`].
const PRIVATE_PROFILE: u8 = 3;
/// Tag of [`Private::Settings`].
const PRIVATE_SETTINGS: u8 = 4;

/// The longest `enc_bundle` an `Invite` carries.
const MAX_ENC_BUNDLE_BYTES: usize = INVITE_BUNDLE_LOG_MAX_BYTES;

/// `KeysError::Corrupted("malformed membership <what>: <reason>")`; static text only.
pub(super) fn malformed(what: &str, reason: &str) -> KeysError {
    KeysError::Corrupted(format!("malformed membership {what}: {reason}"))
}

// ----- the header ----------------------------------------------------------------------------

/// The canonical header bytes.
pub(super) fn encode_header(header: &MembershipHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(160);
    out.push(MEMBERSHIP_VERSION);
    out.extend_from_slice(header.feed.as_bytes());
    out.extend_from_slice(&header.seq.to_le_bytes());
    out.extend_from_slice(&header.prev);
    out.extend_from_slice(header.account.as_bytes());
    out.extend_from_slice(header.device.as_bytes());
    out.push(header.body.kind().to_byte());
    put_fields(&mut out, &header.body);
    out.extend_from_slice(&header.at.to_le_bytes());
    out.extend_from_slice(&header.meta_epoch.to_le_bytes());
    out
}

fn put_fields(out: &mut Vec<u8>, body: &EntryBody) {
    match body {
        EntryBody::Create {
            epoch,
            sk_commitment,
            boxes,
            member_heads,
        } => {
            out.extend_from_slice(&epoch.to_le_bytes());
            out.extend_from_slice(sk_commitment);
            put_boxes(out, boxes);
            put_heads(out, member_heads);
        }
        EntryBody::Invite {
            invite_id,
            invite_pub,
            role,
            expires_at,
            bound_account,
            verified,
            enc_bundle,
        } => {
            out.extend_from_slice(invite_id.as_bytes());
            out.extend_from_slice(&invite_pub.to_bytes());
            out.push(role.to_byte());
            out.extend_from_slice(&expires_at.to_le_bytes());
            match bound_account {
                None => out.push(0),
                Some(account) => {
                    out.push(1);
                    out.extend_from_slice(account.as_bytes());
                }
            }
            out.push(u8::from(*verified));
            match enc_bundle {
                None => out.push(0),
                Some(bundle) => {
                    out.push(1);
                    put_bytes(out, bundle);
                }
            }
        }
        EntryBody::InviteRevoke { invite_id } => out.extend_from_slice(invite_id.as_bytes()),
        EntryBody::Join {
            invite_id,
            proof,
            account,
            log,
        } => {
            out.extend_from_slice(invite_id.as_bytes());
            out.extend_from_slice(&proof.to_bytes());
            out.extend_from_slice(account.as_bytes());
            out.extend_from_slice(&log.seq.to_le_bytes());
            out.extend_from_slice(&log.hash);
        }
        EntryBody::RoleSet { target, role } => {
            out.extend_from_slice(target.as_bytes());
            out.push(role.to_byte());
        }
        EntryBody::Remove { target, rekey } => {
            out.extend_from_slice(target.as_bytes());
            put_rekey(out, rekey);
        }
        EntryBody::Rekey(rekey) => put_rekey(out, rekey),
        EntryBody::Grant {
            member,
            epoch,
            boxes,
        } => {
            out.extend_from_slice(member.as_bytes());
            out.extend_from_slice(&epoch.to_le_bytes());
            put_boxes(out, boxes);
        }
        EntryBody::Leave | EntryBody::Profile | EntryBody::Settings | EntryBody::Close => {}
    }
}

fn put_rekey(out: &mut Vec<u8>, rekey: &RekeyFields) {
    out.extend_from_slice(&rekey.new_epoch.to_le_bytes());
    out.extend_from_slice(&rekey.sk_commitment);
    out.extend_from_slice(&rekey.prev_wrapped.to_bytes());
    put_boxes(out, &rekey.boxes);
    put_heads(out, &rekey.member_heads);
}

/// A `u32` count, cut at `u32::MAX` like [`put_bytes`] so the output stays decodable.
fn count_of(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

fn put_boxes(out: &mut Vec<u8>, boxes: &[KeyBox]) {
    let count = count_of(boxes.len());
    out.extend_from_slice(&count.to_le_bytes());
    for key_box in boxes
        .iter()
        .take(usize::try_from(count).unwrap_or(usize::MAX))
    {
        out.extend_from_slice(key_box.device.as_bytes());
        put_bytes(out, &key_box.bytes);
    }
}

fn put_heads(out: &mut Vec<u8>, heads: &[MemberHead]) {
    let count = count_of(heads.len());
    out.extend_from_slice(&count.to_le_bytes());
    for head in heads
        .iter()
        .take(usize::try_from(count).unwrap_or(usize::MAX))
    {
        out.extend_from_slice(head.account.as_bytes());
        out.extend_from_slice(&head.head.seq.to_le_bytes());
        out.extend_from_slice(&head.head.hash);
    }
}

/// Parses [`encode_header`] output strictly.
pub(super) fn decode_header(bytes: &[u8]) -> Result<MembershipHeader, KeysError> {
    let bad = |error: DecodeError| malformed("header", error.0);
    let mut r = Reader::new(bytes);
    if r.u8().map_err(bad)? != MEMBERSHIP_VERSION {
        return Err(malformed("header", "unknown version"));
    }
    let feed = FeedId::from_bytes(r.array().map_err(bad)?);
    let seq = r.u64().map_err(bad)?;
    let prev = r.array().map_err(bad)?;
    let account = AccountId::from_bytes(r.array().map_err(bad)?);
    let device = DeviceId::from_bytes(r.array().map_err(bad)?);
    let kind_byte = r.u8().map_err(bad)?;
    let kind =
        EntryKind::from_byte(kind_byte).ok_or_else(|| malformed("header", "unknown kind"))?;
    let body = read_fields(&mut r, kind)?;
    let at = r.i64().map_err(bad)?;
    let meta_epoch = r.u32().map_err(bad)?;
    r.finish().map_err(bad)?;
    Ok(MembershipHeader {
        feed,
        seq,
        prev,
        account,
        device,
        at,
        meta_epoch,
        body,
    })
}

fn read_fields(r: &mut Reader<'_>, kind: EntryKind) -> Result<EntryBody, KeysError> {
    let bad = |error: DecodeError| malformed("header", error.0);
    Ok(match kind {
        EntryKind::Create => EntryBody::Create {
            epoch: r.u32().map_err(bad)?,
            sk_commitment: r.array().map_err(bad)?,
            boxes: read_boxes(r)?,
            member_heads: read_heads(r)?,
        },
        EntryKind::Invite => {
            let invite_id = InviteId::from_bytes(r.array().map_err(bad)?);
            let invite_pub = VerifyingKey::from_bytes(&r.array::<32>().map_err(bad)?)
                .map_err(|_| malformed("header", "invalid invite key"))?;
            let role = read_role(r)?;
            let expires_at = r.i64().map_err(bad)?;
            let bound_account = if r.bool().map_err(bad)? {
                Some(AccountId::from_bytes(r.array().map_err(bad)?))
            } else {
                None
            };
            let verified = r.bool().map_err(bad)?;
            let enc_bundle = if r.bool().map_err(bad)? {
                let bundle = r.bytes().map_err(bad)?;
                if bundle.len() > MAX_ENC_BUNDLE_BYTES {
                    return Err(malformed("header", "the invite bundle is too big"));
                }
                Some(bundle.to_vec())
            } else {
                None
            };
            EntryBody::Invite {
                invite_id,
                invite_pub,
                role,
                expires_at,
                bound_account,
                verified,
                enc_bundle,
            }
        }
        EntryKind::InviteRevoke => EntryBody::InviteRevoke {
            invite_id: InviteId::from_bytes(r.array().map_err(bad)?),
        },
        EntryKind::Join => EntryBody::Join {
            invite_id: InviteId::from_bytes(r.array().map_err(bad)?),
            proof: Signature::from_bytes(&r.array::<SIGNATURE_LEN>().map_err(bad)?)
                .map_err(|_| malformed("header", "invalid proof"))?,
            account: AccountId::from_bytes(r.array().map_err(bad)?),
            log: ChainHead {
                seq: r.u64().map_err(bad)?,
                hash: r.array().map_err(bad)?,
            },
        },
        EntryKind::RoleSet => EntryBody::RoleSet {
            target: AccountId::from_bytes(r.array().map_err(bad)?),
            role: read_role(r)?,
        },
        EntryKind::Remove => EntryBody::Remove {
            target: AccountId::from_bytes(r.array().map_err(bad)?),
            rekey: read_rekey(r)?,
        },
        EntryKind::Leave => EntryBody::Leave,
        EntryKind::Rekey => EntryBody::Rekey(read_rekey(r)?),
        EntryKind::Grant => EntryBody::Grant {
            member: AccountId::from_bytes(r.array().map_err(bad)?),
            epoch: r.u32().map_err(bad)?,
            boxes: read_boxes(r)?,
        },
        EntryKind::Profile => EntryBody::Profile,
        EntryKind::Settings => EntryBody::Settings,
        EntryKind::Close => EntryBody::Close,
    })
}

fn read_role(r: &mut Reader<'_>) -> Result<MemberRole, KeysError> {
    let byte = r.u8().map_err(|error| malformed("header", error.0))?;
    MemberRole::from_byte(byte).ok_or_else(|| malformed("header", "unknown role"))
}

fn read_rekey(r: &mut Reader<'_>) -> Result<RekeyFields, KeysError> {
    let bad = |error: DecodeError| malformed("header", error.0);
    Ok(RekeyFields {
        new_epoch: r.u32().map_err(bad)?,
        sk_commitment: r.array().map_err(bad)?,
        prev_wrapped: WrappedKey::from_bytes(r.take(WRAPPED_KEY_LEN).map_err(bad)?)
            .map_err(|_| malformed("header", "invalid wrapped key"))?,
        boxes: read_boxes(r)?,
        member_heads: read_heads(r)?,
    })
}

fn read_boxes(r: &mut Reader<'_>) -> Result<Vec<KeyBox>, KeysError> {
    let bad = |error: DecodeError| malformed("header", error.0);
    let count = r.u32().map_err(bad)?;
    if usize::try_from(count).unwrap_or(usize::MAX) > MAX_BOXES_PER_ENTRY {
        return Err(malformed("header", "too many boxes"));
    }
    let mut boxes = Vec::new();
    for _ in 0..count {
        boxes.push(KeyBox {
            device: DeviceId::from_bytes(r.array().map_err(bad)?),
            bytes: r.bytes().map_err(bad)?.to_vec(),
        });
    }
    Ok(boxes)
}

fn read_heads(r: &mut Reader<'_>) -> Result<Vec<MemberHead>, KeysError> {
    let bad = |error: DecodeError| malformed("header", error.0);
    let count = r.u32().map_err(bad)?;
    if usize::try_from(count).unwrap_or(usize::MAX) > MAX_MEMBERS_PER_FEED {
        return Err(malformed("header", "too many member heads"));
    }
    let mut heads = Vec::new();
    for _ in 0..count {
        heads.push(MemberHead {
            account: AccountId::from_bytes(r.array().map_err(bad)?),
            head: ChainHead {
                seq: r.u64().map_err(bad)?,
                hash: r.array().map_err(bad)?,
            },
        });
    }
    Ok(heads)
}

// ----- the private part ----------------------------------------------------------------------

/// The plaintext sealed under the membership-meta key.
pub(super) fn encode_private(private: &Private) -> Vec<u8> {
    let mut out = Vec::new();
    match private {
        Private::Create {
            space_name,
            settings,
            profile_name,
        } => {
            out.push(PRIVATE_CREATE);
            put_bytes(&mut out, space_name.as_bytes());
            put_settings(&mut out, settings);
            put_bytes(&mut out, profile_name.as_bytes());
        }
        Private::Join { profile_name } => {
            out.push(PRIVATE_JOIN);
            put_bytes(&mut out, profile_name.as_bytes());
        }
        Private::Profile { display_name } => {
            out.push(PRIVATE_PROFILE);
            put_bytes(&mut out, display_name.as_bytes());
        }
        Private::Settings { settings } => {
            out.push(PRIVATE_SETTINGS);
            put_settings(&mut out, settings);
        }
        Private::None => {}
    }
    out
}

fn put_settings(out: &mut Vec<u8>, settings: &SpaceSettings) {
    out.extend_from_slice(&settings.trash_retention_days.to_le_bytes());
    out.push(u8::from(settings.editors_may_link));
}

/// Parses the plaintext of an entry of `kind`: the kinds with private fields need their tag, the
/// others an empty plaintext.
pub(super) fn decode_private(kind: EntryKind, bytes: &[u8]) -> Result<Private, KeysError> {
    let bad = |error: DecodeError| malformed("private part", error.0);
    let expected = match kind {
        EntryKind::Create => Some(PRIVATE_CREATE),
        EntryKind::Join => Some(PRIVATE_JOIN),
        EntryKind::Profile => Some(PRIVATE_PROFILE),
        EntryKind::Settings => Some(PRIVATE_SETTINGS),
        _ => None,
    };
    let Some(expected) = expected else {
        return if bytes.is_empty() {
            Ok(Private::None)
        } else {
            Err(malformed("private part", "this kind has none"))
        };
    };
    let mut r = Reader::new(bytes);
    if r.u8().map_err(bad)? != expected {
        return Err(malformed("private part", "it does not match the kind"));
    }
    let private = match expected {
        PRIVATE_CREATE => Private::Create {
            space_name: read_text(&mut r)?,
            settings: read_settings(&mut r)?,
            profile_name: read_name(&mut r)?,
        },
        PRIVATE_JOIN => Private::Join {
            profile_name: read_name(&mut r)?,
        },
        PRIVATE_PROFILE => Private::Profile {
            display_name: read_name(&mut r)?,
        },
        _ => Private::Settings {
            settings: read_settings(&mut r)?,
        },
    };
    r.finish().map_err(bad)?;
    Ok(private)
}

fn read_text(r: &mut Reader<'_>) -> Result<String, KeysError> {
    let raw = r
        .bytes()
        .map_err(|error| malformed("private part", error.0))?;
    core::str::from_utf8(raw)
        .map(str::to_owned)
        .map_err(|_| malformed("private part", "text is not UTF-8"))
}

/// A profile name: text of 1 to `PROFILE_NAME_MAX_CHARS` characters.
fn read_name(r: &mut Reader<'_>) -> Result<String, KeysError> {
    let name = read_text(r)?;
    let chars = name.chars().count();
    if chars == 0 || chars > PROFILE_NAME_MAX_CHARS {
        return Err(malformed("private part", "the name has the wrong length"));
    }
    Ok(name)
}

fn read_settings(r: &mut Reader<'_>) -> Result<SpaceSettings, KeysError> {
    let bad = |error: DecodeError| malformed("private part", error.0);
    Ok(SpaceSettings {
        trash_retention_days: r.u32().map_err(bad)?,
        editors_may_link: r.bool().map_err(bad)?,
    })
}
