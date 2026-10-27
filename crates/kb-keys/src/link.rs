//! The keys of a view link (M13 slice 4, ADR-0051, `docs/security/sharing.md` section 10,
//! `docs/development/milestone-13.md` section 4.1): the link key `K_link` that travels in the URL
//! fragment, the link id and the publication key derived from it, and the text form of the key.
//!
//! ```text
//! K_link          32 random bytes (never stored on the server)
//! link id         HKDF(K_link, "link-id")[..16]          (the server's name for the link)
//! publication key HKDF(K_link, "publication")            (`kb_sync::v2::seal_publication`)
//! fragment        base64url(K_link), no padding          (43 characters; `<server>/view#<fragment>`)
//! ```
//!
//! Whoever holds `K_link` reads the link's page; the id derived from it tells the server nothing about
//! the key. Member devices keep `K_link` as a `Link` record of the keyring
//! ([`crate::keyring::wrap_link_key`]), wrapped under the space key, so any member can refresh the
//! link or show its URL again.

use kb_crypto::{Label, Secret32};
use kb_types::{LinkId, Rng};
use zeroize::Zeroizing;

use crate::KeysError;

/// Characters of the fragment form of a link key: 256 bits in groups of 6, the last character
/// carrying four bits and two zero bits.
pub const LINK_KEY_FRAGMENT_LEN: usize = 43;

/// The base64url alphabet (RFC 4648 section 5), without padding.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The link key `K_link`: 32 random bytes. Never printed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LinkKey(Secret32);

impl LinkKey {
    /// A fresh link key.
    #[must_use]
    pub fn generate(rng: &dyn Rng) -> Self {
        Self(Secret32::generate(rng))
    }

    /// Wraps key bytes (unwrapped from a keyring record, or decoded from a fragment).
    #[must_use]
    pub const fn from_secret(secret: Secret32) -> Self {
        Self(secret)
    }

    /// The underlying secret. Keep the borrow short; never log it.
    #[must_use]
    pub fn secret(&self) -> &Secret32 {
        &self.0
    }

    /// The link id, `HKDF(K_link, Label::LINK_ID)[..16]`.
    #[must_use]
    pub fn link_id(&self) -> LinkId {
        link_id(&self.0)
    }

    /// The publication key, `HKDF(K_link, Label::PUBLICATION)`.
    #[must_use]
    pub fn publication_key(&self) -> Secret32 {
        publication_key(&self.0)
    }

    /// The fragment form: base64url without padding, [`LINK_KEY_FRAGMENT_LEN`] characters (what follows
    /// the `#` of the link's URL). The text is the key: show it only to people who may copy it, and
    /// never log it.
    #[must_use]
    pub fn to_fragment(&self) -> String {
        let mut out = String::with_capacity(LINK_KEY_FRAGMENT_LEN);
        let bytes = self.0.expose();
        for group in bytes.chunks(3) {
            let [a, b, c] = match *group {
                [a, b, c] => [a, b, c],
                [a, b] => [a, b, 0],
                [a] => [a, 0, 0],
                _ => continue,
            };
            let value = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
            // 3 bytes make 4 characters; a group of 2 makes 3 and a group of 1 makes 2.
            let characters = match group.len() {
                3 => 4,
                2 => 3,
                _ => 2,
            };
            for index in 0..characters {
                let shift = 18 - 6 * index;
                out.push(symbol((value >> shift) & 0x3f));
            }
        }
        out
    }

    /// Parses [`Self::to_fragment`] output; a leading `#` is accepted (it is what `location.hash`
    /// holds). Exactly [`LINK_KEY_FRAGMENT_LEN`] characters of the base64url alphabet whose unused
    /// trailing bits are zero (one spelling per key); anything else is [`KeysError::Corrupted`] with a
    /// message that never repeats the text.
    pub fn from_fragment(text: &str) -> Result<Self, KeysError> {
        let text = text.strip_prefix('#').unwrap_or(text);
        let malformed =
            || KeysError::Corrupted("a view link key has 43 letters, digits, - and _".to_owned());
        if text.len() != LINK_KEY_FRAGMENT_LEN {
            return Err(malformed());
        }
        let mut bytes = Zeroizing::new([0u8; 32]);
        let mut next = 0usize;
        let mut buffer: u32 = 0;
        let mut bits = 0u32;
        for character in text.bytes() {
            let value = value_of(character).ok_or_else(malformed)?;
            buffer = (buffer << 6) | u32::from(value);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                let byte = u8::try_from(buffer >> bits).map_err(|_| malformed())?;
                let slot = bytes.get_mut(next).ok_or_else(malformed)?;
                *slot = byte;
                next += 1;
                buffer &= (1 << bits) - 1;
            }
        }
        // 43 characters leave 2 bits that must be zero, and every byte must have been filled.
        if next != 32 {
            return Err(malformed());
        }
        if buffer != 0 {
            return Err(KeysError::Corrupted(
                "a view link key is not canonically encoded".to_owned(),
            ));
        }
        Ok(Self(Secret32::from_bytes(*bytes)))
    }
}

/// The base64url character of the low six bits of `value`.
fn symbol(value: u32) -> char {
    let index = usize::try_from(value & 0x3f).unwrap_or(0);
    ALPHABET.get(index).map_or('A', |&byte| char::from(byte))
}

/// The six-bit value of a base64url character.
fn value_of(character: u8) -> Option<u8> {
    match character {
        b'A'..=b'Z' => Some(character - b'A'),
        b'a'..=b'z' => Some(character - b'a' + 26),
        b'0'..=b'9' => Some(character - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// The link id of a link key: `HKDF(K_link, Label::LINK_ID, "")[..16]`.
#[must_use]
pub fn link_id(link_key: &Secret32) -> LinkId {
    let okm = kb_crypto::derive_key(link_key, Label::LINK_ID, b"");
    let mut id = [0u8; 16];
    id.copy_from_slice(&okm.expose()[..16]);
    LinkId::from_bytes(id)
}

/// The key a link's publication is sealed under: `HKDF(K_link, Label::PUBLICATION, "")`.
#[must_use]
pub fn publication_key(link_key: &Secret32) -> Secret32 {
    kb_crypto::derive_key(link_key, Label::PUBLICATION, b"")
}

#[cfg(test)]
mod tests;
