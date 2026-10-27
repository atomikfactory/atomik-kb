//! Invites to a shared space (M13, ADR-0046, `docs/security/sharing.md` section 7,
//! `docs/development/milestone-13.md` section 1.2): the invite secret `s`, the id and the signing
//! key derived from it, the bundles handed to an invitee, and the signatures an invitee makes with
//! the invite key (the join proof, the signup claim, the open request).
//!
//! `s` is 32 random bytes. `invite_id = HKDF(s, "invite-id")[..16]`; the invite's Ed25519 seed is
//! `HKDF(s, "invite-sign")`; a link invite's bundle is sealed under `HKDF(s, "invite-bundle")`. A
//! link carries `s` in its fragment (`#<s base32>`); a code invite carries it inside the sealed
//! bundle of the code flow. The server never sees `s`: it holds the invite's public key.
//!
//! All signatures here use `kb_crypto::sign` with the context [`INVITE_CONTEXT`] and these
//! messages (fixed-length fields, so no ambiguity):
//!
//! ```text
//! join proof   feed ‖ invite_id ‖ account ‖ u64 log_seq LE ‖ [32] log_hash
//! signup       "signup" ‖ account ‖ server_epoch
//! open         "open" ‖ invite_id ‖ server_epoch
//! ```
//!
//! # Bundles (version 1, little-endian, strict)
//!
//! ```text
//! link bundle   u8 1 | u32 epoch | [32] space key | u32 len + space name | u32 len + inviter name
//! code bundle   u8 1 | [16] feed | [16] space | [16] inviter | [32] s | u32 epoch | [32] space key |
//!               u64 membership head seq | [32] membership head hash | u32 len + space name
//! enc_bundle    [24] nonce | AEAD(HKDF(s, "invite-bundle"), nonce, aad = invite id, link bundle)
//! ```

use kb_crypto::{
    CryptoError, NONCE_LEN, Nonce24, Secret32, Signature, SigningKey, TAG_LEN, VerifyingKey,
};
use kb_types::{AccountId, FeedId, InviteId, Rng, SpaceId};
use zeroize::Zeroizing;

use crate::KeysError;
use crate::codec::{DecodeError, Reader, put_bytes};
use crate::membership::ChainHead;
use crate::pairing::{CROCKFORD, is_dash, symbol_value};
use crate::space_keys::SpaceKey;

/// Version byte of the link and code bundles.
const BUNDLE_VERSION: u8 = 1;
/// Characters of the base32 form of a secret: 256 bits in groups of 5, the last group padded with
/// four zero bits.
const BASE32_LEN: usize = 52;

/// The signature context of the invite signatures: `kb_types::sharing::INVITE_TRANSCRIPT_DOMAIN`
/// without its `atomik-kb/v1/` prefix.
pub const INVITE_CONTEXT: &str = "invite";

/// The invite secret `s`: 32 random bytes. Never printed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InviteSecret(Secret32);

impl InviteSecret {
    /// A fresh secret.
    #[must_use]
    pub fn generate(rng: &dyn Rng) -> Self {
        Self(Secret32::generate(rng))
    }

    /// Wraps secret bytes (from a bundle or a decoded link).
    #[must_use]
    pub const fn from_secret(secret: Secret32) -> Self {
        Self(secret)
    }

    /// The underlying secret. Keep the borrow short; never log it.
    #[must_use]
    pub fn secret(&self) -> &Secret32 {
        &self.0
    }

    /// The link form: Crockford base32 (the alphabet of the pairing code), upper case, no padding, no
    /// separators: 52 characters (the 256 bits, most significant first, then four zero bits).
    #[must_use]
    pub fn to_base32(&self) -> String {
        let mut out = String::with_capacity(BASE32_LEN);
        let mut buffer: u16 = 0;
        let mut bits = 0u32;
        for byte in self.0.expose() {
            buffer = (buffer << 8) | u16::from(*byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(base32_symbol(buffer >> bits));
            }
            buffer &= (1 << bits) - 1;
        }
        if bits > 0 {
            out.push(base32_symbol(buffer << (5 - bits)));
        }
        out
    }

    /// Parses [`Self::to_base32`] output: case-insensitive, whitespace and dashes ignored, `O` read
    /// as `0` and `I`, `L` as `1`; exactly 52 symbols whose unused trailing bits are zero. Anything
    /// else is [`KeysError::Corrupted`] with a message that never repeats the text.
    pub fn from_base32(text: &str) -> Result<Self, KeysError> {
        let wrong_length = || {
            KeysError::Corrupted(format!(
                "an invite secret has {BASE32_LEN} letters and digits"
            ))
        };
        let mut bytes = Zeroizing::new([0u8; 32]);
        let mut next = 0usize;
        let mut buffer: u16 = 0;
        let mut bits = 0u32;
        let mut symbols = 0usize;
        for c in text.chars() {
            if c.is_whitespace() || is_dash(c) {
                continue;
            }
            let value = symbol_value(c).ok_or_else(|| {
                KeysError::Corrupted(
                    "an invite secret has only digits and the letters A to Z, without U".to_owned(),
                )
            })?;
            symbols += 1;
            if symbols > BASE32_LEN {
                return Err(wrong_length());
            }
            buffer = (buffer << 5) | u16::from(value);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                let byte = u8::try_from(buffer >> bits).map_err(|_| wrong_length())?;
                if let Some(slot) = bytes.get_mut(next) {
                    *slot = byte;
                    next += 1;
                }
                buffer &= (1 << bits) - 1;
            }
        }
        // 52 symbols leave 4 bits that must be zero, and every byte must have been filled.
        if symbols != BASE32_LEN || next != 32 {
            return Err(wrong_length());
        }
        if buffer != 0 {
            return Err(KeysError::Corrupted(
                "an invite secret is not canonically encoded".to_owned(),
            ));
        }
        Ok(Self(Secret32::from_bytes(*bytes)))
    }
}

/// The Crockford symbol of the low five bits of `value`.
fn base32_symbol(value: u16) -> char {
    CROCKFORD
        .get(usize::from(value & 0x1f))
        .map_or('0', |&byte| char::from(byte))
}

/// `HKDF(s, Label::INVITE_ID)[..16]`.
#[must_use]
pub fn invite_id(secret: &InviteSecret) -> InviteId {
    let okm = kb_crypto::derive_key(&secret.0, kb_crypto::Label::INVITE_ID, b"");
    let mut id = [0u8; 16];
    id.copy_from_slice(&okm.expose()[..16]);
    InviteId::from_bytes(id)
}

/// The invite's signing key: the Ed25519 key of the seed `HKDF(s, Label::INVITE_SIGN)`.
#[must_use]
pub fn invite_signing_key(secret: &InviteSecret) -> SigningKey {
    let seed = kb_crypto::derive_key(&secret.0, kb_crypto::Label::INVITE_SIGN, b"");
    SigningKey::from_seed(&seed)
}

/// The key of a link invite's sealed bundle: `HKDF(s, Label::INVITE_BUNDLE)`.
#[must_use]
pub fn invite_bundle_key(secret: &InviteSecret) -> Secret32 {
    kb_crypto::derive_key(&secret.0, kb_crypto::Label::INVITE_BUNDLE, b"")
}

/// `KeysError::Corrupted("malformed invite <what>: <reason>")`; static text only.
fn malformed(what: &str, error: DecodeError) -> KeysError {
    KeysError::Corrupted(format!("malformed invite {what}: {}", error.0))
}

/// UTF-8 text out of a length-prefixed field.
fn text(raw: &[u8], what: &str) -> Result<String, KeysError> {
    core::str::from_utf8(raw)
        .map(str::to_owned)
        .map_err(|_| KeysError::Corrupted(format!("malformed invite {what}: not UTF-8")))
}

/// What a link invite carries for the invitee (sealed into the `Invite` entry's `enc_bundle`): the
/// key and the names for the preview. The feed and the membership log come from
/// `invites/{id}/open`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LinkBundle {
    /// The epoch of `space_key`.
    pub epoch: u32,
    /// The space key of that epoch.
    pub space_key: SpaceKey,
    /// The space's name.
    pub space_name: String,
    /// The inviter's display name.
    pub inviter_name: String,
}

impl LinkBundle {
    /// Strict length-prefixed encoding (the key bytes included: only ever sealed); layout in the
    /// [module documentation](self).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(1 + 4 + 32 + 8 + self.space_name.len() + self.inviter_name.len());
        out.push(BUNDLE_VERSION);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(self.space_key.secret().expose());
        put_bytes(&mut out, self.space_name.as_bytes());
        put_bytes(&mut out, self.inviter_name.as_bytes());
        out
    }

    /// Parses [`Self::encode`]; truncation, trailing bytes and text that is not UTF-8 are
    /// [`KeysError::Corrupted`].
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let bad = |error: DecodeError| malformed("link bundle", error);
        let mut r = Reader::new(bytes);
        if r.u8().map_err(bad)? != BUNDLE_VERSION {
            return Err(KeysError::Corrupted(
                "malformed invite link bundle: unknown version".to_owned(),
            ));
        }
        let epoch = r.u32().map_err(bad)?;
        let key = Zeroizing::new(r.array::<32>().map_err(bad)?);
        let space_name = text(r.bytes().map_err(bad)?, "link bundle")?;
        let inviter_name = text(r.bytes().map_err(bad)?, "link bundle")?;
        r.finish().map_err(bad)?;
        Ok(Self {
            epoch,
            space_key: SpaceKey::from_secret(Secret32::from_bytes(*key)),
            space_name,
            inviter_name,
        })
    }
}

/// Seals a link bundle for the `Invite` entry's `enc_bundle` under [`invite_bundle_key`]
/// (XChaCha20-Poly1305, a random nonce, the invite id as AAD): `nonce ‖ ciphertext ‖ tag`.
#[must_use]
pub fn seal_link_bundle(bundle: &LinkBundle, secret: &InviteSecret, rng: &dyn Rng) -> Vec<u8> {
    let nonce = Nonce24::random(rng);
    let plaintext = Zeroizing::new(bundle.encode());
    let ciphertext = kb_crypto::aead_seal(
        &invite_bundle_key(secret),
        &nonce,
        invite_id(secret).as_bytes(),
        &plaintext,
    );
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce.as_bytes());
    out.extend_from_slice(&ciphertext);
    out
}

/// Reverses [`seal_link_bundle`]: a wrong secret or tampering is [`KeysError::Crypto`]
/// (`Authentication`); input that cannot hold a nonce and a tag is [`KeysError::Crypto`]
/// (`Encoding`).
pub fn open_link_bundle(bytes: &[u8], secret: &InviteSecret) -> Result<LinkBundle, KeysError> {
    let too_short = || {
        KeysError::Crypto(CryptoError::Encoding(
            "a sealed link bundle is too short".to_owned(),
        ))
    };
    let (nonce, ciphertext) = bytes
        .split_first_chunk::<NONCE_LEN>()
        .ok_or_else(too_short)?;
    if ciphertext.len() < TAG_LEN {
        return Err(too_short());
    }
    let plaintext = kb_crypto::aead_open(
        &invite_bundle_key(secret),
        &Nonce24::from_bytes(*nonce),
        invite_id(secret).as_bytes(),
        ciphertext,
    )?;
    LinkBundle::decode(&plaintext)
}

/// What the inviter sends the invitee over the code flow's sealed channel (`MessageKind::
/// InviteBundle`): everything the invitee needs to verify the log and join.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CodeBundle {
    /// The shared feed.
    pub feed_id: FeedId,
    /// The space (the invitee's device creates the local space under this id).
    pub space_id: SpaceId,
    /// The inviting Owner's account.
    pub inviter: AccountId,
    /// The invite secret (the invite is bound to the invitee's account).
    pub secret: InviteSecret,
    /// The epoch of `space_key`.
    pub epoch: u32,
    /// The space key of that epoch.
    pub space_key: SpaceKey,
    /// The head of the membership log including the `Invite` (the invitee checks the log it fetches
    /// matches or extends it).
    pub membership_head: ChainHead,
    /// The space's name.
    pub space_name: String,
}

impl CodeBundle {
    /// Strict length-prefixed encoding (the secrets included: only ever sealed in the session's
    /// channel); layout in the [module documentation](self).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(1 + 16 * 3 + 32 + 4 + 32 + 8 + 32 + 4 + self.space_name.len());
        out.push(BUNDLE_VERSION);
        out.extend_from_slice(self.feed_id.as_bytes());
        out.extend_from_slice(self.space_id.as_bytes());
        out.extend_from_slice(self.inviter.as_bytes());
        out.extend_from_slice(self.secret.0.expose());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(self.space_key.secret().expose());
        out.extend_from_slice(&self.membership_head.seq.to_le_bytes());
        out.extend_from_slice(&self.membership_head.hash);
        put_bytes(&mut out, self.space_name.as_bytes());
        out
    }

    /// Parses [`Self::encode`]; truncation, trailing bytes and text that is not UTF-8 are
    /// [`KeysError::Corrupted`].
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let bad = |error: DecodeError| malformed("code bundle", error);
        let mut r = Reader::new(bytes);
        if r.u8().map_err(bad)? != BUNDLE_VERSION {
            return Err(KeysError::Corrupted(
                "malformed invite code bundle: unknown version".to_owned(),
            ));
        }
        let feed_id = FeedId::from_bytes(r.array().map_err(bad)?);
        let space_id = SpaceId::from_bytes(r.array().map_err(bad)?);
        let inviter = AccountId::from_bytes(r.array().map_err(bad)?);
        let secret = Zeroizing::new(r.array::<32>().map_err(bad)?);
        let epoch = r.u32().map_err(bad)?;
        let key = Zeroizing::new(r.array::<32>().map_err(bad)?);
        let membership_head = ChainHead {
            seq: r.u64().map_err(bad)?,
            hash: r.array().map_err(bad)?,
        };
        let space_name = text(r.bytes().map_err(bad)?, "code bundle")?;
        r.finish().map_err(bad)?;
        Ok(Self {
            feed_id,
            space_id,
            inviter,
            secret: InviteSecret::from_secret(Secret32::from_bytes(*secret)),
            epoch,
            space_key: SpaceKey::from_secret(Secret32::from_bytes(*key)),
            membership_head,
            space_name,
        })
    }
}

/// The message of a join proof: `feed ‖ invite_id ‖ account ‖ u64 log_seq LE ‖ [32] log_hash`.
fn join_proof_message(
    feed: FeedId,
    invite_id: InviteId,
    account: AccountId,
    log: &ChainHead,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(16 * 3 + 8 + 32);
    message.extend_from_slice(feed.as_bytes());
    message.extend_from_slice(invite_id.as_bytes());
    message.extend_from_slice(account.as_bytes());
    message.extend_from_slice(&log.seq.to_le_bytes());
    message.extend_from_slice(&log.hash);
    message
}

/// Signs the proof of a `Join` with the invite key: `feed ‖ invite_id ‖ account ‖ log_seq ‖
/// log_hash` (context [`INVITE_CONTEXT`]).
#[must_use]
pub fn sign_join_proof(
    key: &SigningKey,
    feed: FeedId,
    invite_id: InviteId,
    account: AccountId,
    log: &ChainHead,
) -> Signature {
    kb_crypto::sign(
        key,
        INVITE_CONTEXT,
        &join_proof_message(feed, invite_id, account, log),
    )
}

/// Verifies a join proof against the invite's public key (the server and the fold do): a mismatch
/// is [`KeysError::Crypto`] (`Authentication`).
pub fn verify_join_proof(
    invite_pub: &VerifyingKey,
    feed: FeedId,
    invite_id: InviteId,
    account: AccountId,
    log: &ChainHead,
    proof: &Signature,
) -> Result<(), KeysError> {
    kb_crypto::verify(
        invite_pub,
        INVITE_CONTEXT,
        &join_proof_message(feed, invite_id, account, log),
        proof,
    )?;
    Ok(())
}

/// `"signup" ‖ account ‖ server_epoch`.
fn signup_message(account: AccountId, server_epoch: [u8; 16]) -> Vec<u8> {
    let mut message = Vec::with_capacity(6 + 16 + 16);
    message.extend_from_slice(b"signup");
    message.extend_from_slice(account.as_bytes());
    message.extend_from_slice(&server_epoch);
    message
}

/// Signs a share signup claim: `"signup" ‖ account ‖ server_epoch`, what `POST /v2/accounts` with
/// `share {invite_id, sig}` takes.
#[must_use]
pub fn sign_signup(key: &SigningKey, account: AccountId, server_epoch: [u8; 16]) -> Signature {
    kb_crypto::sign(key, INVITE_CONTEXT, &signup_message(account, server_epoch))
}

/// Verifies a signup claim (the server does): a mismatch is [`KeysError::Crypto`]
/// (`Authentication`).
pub fn verify_signup(
    invite_pub: &VerifyingKey,
    account: AccountId,
    server_epoch: [u8; 16],
    sig: &Signature,
) -> Result<(), KeysError> {
    kb_crypto::verify(
        invite_pub,
        INVITE_CONTEXT,
        &signup_message(account, server_epoch),
        sig,
    )?;
    Ok(())
}

/// `"open" ‖ invite_id ‖ server_epoch`.
fn open_message(invite_id: InviteId, server_epoch: [u8; 16]) -> Vec<u8> {
    let mut message = Vec::with_capacity(4 + 16 + 16);
    message.extend_from_slice(b"open");
    message.extend_from_slice(invite_id.as_bytes());
    message.extend_from_slice(&server_epoch);
    message
}

/// Signs an `invites/{id}/open` request: `"open" ‖ invite_id ‖ server_epoch`.
#[must_use]
pub fn sign_open(key: &SigningKey, invite_id: InviteId, server_epoch: [u8; 16]) -> Signature {
    kb_crypto::sign(key, INVITE_CONTEXT, &open_message(invite_id, server_epoch))
}

/// Verifies an open request (the server does): a mismatch is [`KeysError::Crypto`]
/// (`Authentication`).
pub fn verify_open(
    invite_pub: &VerifyingKey,
    invite_id: InviteId,
    server_epoch: [u8; 16],
    sig: &Signature,
) -> Result<(), KeysError> {
    kb_crypto::verify(
        invite_pub,
        INVITE_CONTEXT,
        &open_message(invite_id, server_epoch),
        sig,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
