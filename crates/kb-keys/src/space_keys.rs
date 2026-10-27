//! The keys of a shared space (M13, ADR-0044, `docs/security/sharing.md` section 4,
//! `docs/development/milestone-13.md` section 1.2): the space key `SK[feed][epoch]` as a type, the
//! boxes that carry it to each member device, the backward key chain, and the keys derived from it
//! (membership metadata, the space keyring).
//!
//! A member stores every `SK` it opens as a `Space(epoch)` record of its personal keyring
//! (`keyring::new_space_key`-style, under the ARK), so later devices, backups and ARK rotations
//! carry it. A removed member keeps what it already holds and nothing newer.
//!
//! # Formats
//!
//! ```text
//! box context      "atomik-kb/v1/space-key" "/" <feed: 32 lowercase hex> "/" <epoch: decimal>
//! box              kb_crypto::seal_to(device agreement key, box context, SK[32])   (104 bytes)
//! prev_wrapped     wrap_key(SK[e-1], kek = SK[e], subject = feed[16] ‖ u32 LE (e-1))  (72 bytes)
//! MK               HKDF(SK, "membership-meta", info = feed[16])
//! keyring key      HKDF(SK, "space-keyring", info = feed[16])
//! presence key     HKDF(SK, "presence", info = feed[16])                 (slice 3)
//! ```

use kb_crypto::{CryptoError, Label, Secret32, WrappedKey};
use kb_types::sharing::SPACE_KEY_CONTEXT;
use kb_types::{FeedId, Rng};

use crate::KeysError;
use crate::device::{DeviceKeys, DevicePublicKeys};

/// A shared space's key for one epoch. Never printed (the secret redacts itself).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpaceKey(Secret32);

impl SpaceKey {
    /// Wraps key bytes (opened from a box, unwrapped from a keyring record or from a bundle).
    #[must_use]
    pub const fn from_secret(secret: Secret32) -> Self {
        Self(secret)
    }

    /// A fresh random key.
    #[must_use]
    pub fn generate(rng: &dyn Rng) -> Self {
        Self(Secret32::generate(rng))
    }

    /// The underlying secret. Keep the borrow short; never log it.
    #[must_use]
    pub fn secret(&self) -> &Secret32 {
        &self.0
    }

    /// `kb_crypto::sk_commitment(feed, epoch, self)`.
    #[must_use]
    pub fn commitment(&self, feed: FeedId, epoch: u32) -> [u8; 32] {
        kb_crypto::sk_commitment(feed, epoch, &self.0)
    }
}

/// The context of a space key box: `SPACE_KEY_CONTEXT/<feed hex>/<epoch>` (`<feed hex>` is 32
/// lowercase hex characters, `<epoch>` in decimal), the pattern of `rotation::seal_root_for`.
#[must_use]
pub fn space_key_context(feed: FeedId, epoch: u32) -> String {
    format!("{SPACE_KEY_CONTEXT}/{}/{epoch}", feed.to_hex())
}

/// Seals `sk` to one device (`kb_crypto::seal_to` with [`space_key_context`]). The box does not
/// say who sealed it: the membership entry that carries it is signed.
///
/// A recipient key that cannot be used (it never happens with a key read from a verified device
/// log) gives an empty vector, which no device opens: the closed failure of `seal_to`.
#[must_use]
pub fn seal_space_key_for(
    sk: &SpaceKey,
    feed: FeedId,
    epoch: u32,
    recipient: &DevicePublicKeys,
    rng: &dyn Rng,
) -> Vec<u8> {
    kb_crypto::seal_to(
        &recipient.agreement,
        &space_key_context(feed, epoch),
        sk.0.expose(),
        rng,
    )
}

/// Opens a box made for this device with [`seal_space_key_for`]: a wrong device, feed or epoch, or
/// tampering, is [`KeysError::Crypto`] (`Authentication`); the caller compares the key with the
/// entry's commitment (`membership::verify_for_recipient` does).
pub fn open_space_key_box(
    keys: &DeviceKeys,
    feed: FeedId,
    epoch: u32,
    bytes: &[u8],
) -> Result<SpaceKey, KeysError> {
    let plaintext =
        kb_crypto::open_sealed(keys.agreement(), &space_key_context(feed, epoch), bytes)?;
    let key: &[u8; 32] = plaintext.as_slice().try_into().map_err(|_| {
        KeysError::Crypto(CryptoError::Encoding(
            "a space key box holds 32 bytes".to_owned(),
        ))
    })?;
    Ok(SpaceKey(Secret32::from_bytes(*key)))
}

/// The AAD subject of the backward chain: `feed ‖ epoch_prev` (16 bytes and a little-endian `u32`).
fn prev_subject(feed: FeedId, epoch_prev: u32) -> [u8; 20] {
    let mut subject = [0u8; 20];
    subject[..16].copy_from_slice(feed.as_bytes());
    subject[16..].copy_from_slice(&epoch_prev.to_le_bytes());
    subject
}

/// The backward key chain: `SK[e-1]` wrapped under `SK[e]` with AAD `feed ‖ (e-1)` (the feed as its
/// 16 bytes, the epoch as a little-endian `u32`), the `prev_wrapped` of a `Rekey`. The newest key
/// opens every older epoch, so a joiner gets the whole history.
#[must_use]
pub fn wrap_prev(
    sk_prev: &SpaceKey,
    sk: &SpaceKey,
    feed: FeedId,
    epoch_prev: u32,
    rng: &dyn Rng,
) -> WrappedKey {
    kb_crypto::wrap_key(&sk_prev.0, &sk.0, &prev_subject(feed, epoch_prev), rng)
}

/// Reverses [`wrap_prev`] with the newer key: a wrong key, feed or epoch, or tampering, is
/// [`KeysError::Crypto`] (`Authentication`).
pub fn unwrap_prev(
    sk: &SpaceKey,
    wrapped: &WrappedKey,
    feed: FeedId,
    epoch_prev: u32,
) -> Result<SpaceKey, KeysError> {
    let key = kb_crypto::unwrap_key(wrapped, &sk.0, &prev_subject(feed, epoch_prev))?;
    Ok(SpaceKey(key))
}

/// `MK = HKDF(SK, Label::MEMBERSHIP_META, info = feed id bytes)`: the key of the private part of
/// membership entries (`MembershipEntry::build`, `open_private`).
#[must_use]
pub fn membership_meta_key(sk: &SpaceKey, feed: FeedId) -> Secret32 {
    kb_crypto::derive_key(&sk.0, Label::MEMBERSHIP_META, feed.as_bytes())
}

/// `HKDF(SK, Label::SPACE_KEYRING, info = feed id bytes)`: the transport key of the keyring entries
/// of a shared feed (`kb_sync::seal_keyring` with this key; `Doc`, `Attachment` and tombstone
/// records only, never `Space` records).
#[must_use]
pub fn space_keyring_key(sk: &SpaceKey, feed: FeedId) -> Secret32 {
    kb_crypto::derive_key(&sk.0, Label::SPACE_KEYRING, feed.as_bytes())
}

/// `HKDF(SK, Label::PRESENCE, info = feed id bytes)`: the key presence frames of a shared feed are
/// sealed under (`kb_sync::v2::seal_presence`, M13 slice 3, ADR-0050). One key per space key epoch, so
/// a frame sealed under an epoch the receiver does not hold cannot be opened (and is dropped), and a
/// removed member, who never gets the next epoch's space key, reads no later frame.
#[must_use]
pub fn presence_key(sk: &SpaceKey, feed: FeedId) -> Secret32 {
    kb_crypto::derive_key(&sk.0, Label::PRESENCE, feed.as_bytes())
}

#[cfg(test)]
mod tests;
