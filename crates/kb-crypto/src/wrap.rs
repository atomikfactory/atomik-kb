//! Key wrapping: a 32-byte key sealed under a key-encryption key with the subject
//! in the AAD (`encryption-design.md`, "Key wrapping").

use kb_types::Rng;

use crate::aead::{self, NONCE_LEN, Nonce24, TAG_LEN};
use crate::{CryptoError, Secret32};

/// Domain prefix of the wrapping AAD: `atomik-kb/v1/wrap` || subject.
pub const WRAP_AAD_PREFIX: &[u8] = b"atomik-kb/v1/wrap";
/// Encoded size of a wrapped key: nonce + 32-byte ciphertext + tag.
pub const WRAPPED_KEY_LEN: usize = NONCE_LEN + 32 + TAG_LEN;

/// A wrapped 32-byte key: `nonce || ciphertext || tag`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WrappedKey {
    /// Random nonce used for the wrap.
    pub nonce: Nonce24,
    /// 32 bytes of ciphertext followed by the 16-byte tag.
    pub ciphertext: [u8; 32 + TAG_LEN],
}

impl WrappedKey {
    /// `nonce || ciphertext || tag`, [`WRAPPED_KEY_LEN`] bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; WRAPPED_KEY_LEN] {
        let mut out = [0u8; WRAPPED_KEY_LEN];
        out[..NONCE_LEN].copy_from_slice(self.nonce.as_bytes());
        out[NONCE_LEN..].copy_from_slice(&self.ciphertext);
        out
    }

    /// Parses [`WRAPPED_KEY_LEN`] bytes; anything else is [`CryptoError::Encoding`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let wrong_length = || {
            CryptoError::Encoding(format!(
                "a wrapped key is {WRAPPED_KEY_LEN} bytes, got {}",
                bytes.len()
            ))
        };
        if bytes.len() != WRAPPED_KEY_LEN {
            return Err(wrong_length());
        }
        let (nonce, ciphertext) = bytes
            .split_first_chunk::<NONCE_LEN>()
            .ok_or_else(wrong_length)?;
        let ciphertext: [u8; 32 + TAG_LEN] = ciphertext.try_into().map_err(|_| wrong_length())?;
        Ok(Self {
            nonce: Nonce24::from_bytes(*nonce),
            ciphertext,
        })
    }
}

/// `WRAP_AAD_PREFIX || subject`. The prefix has a fixed length, so the subject is
/// recovered unambiguously.
fn wrap_aad(subject: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(WRAP_AAD_PREFIX.len() + subject.len());
    aad.extend_from_slice(WRAP_AAD_PREFIX);
    aad.extend_from_slice(subject);
    aad
}

/// `wrap(key, kek)`: XChaCha20-Poly1305 of the key under `kek`, random nonce, AAD =
/// [`WRAP_AAD_PREFIX`] || `subject`. The subject names what the key is for (a space
/// id and epoch, a document id, a recovery id, ...), so a wrapped key cannot be
/// transplanted to another subject.
#[must_use]
pub fn wrap_key(key: &Secret32, kek: &Secret32, subject: &[u8], rng: &dyn Rng) -> WrappedKey {
    let nonce = Nonce24::random(rng);
    let sealed = aead::seal(kek, &nonce, &wrap_aad(subject), key.expose());
    // A 32-byte key always seals to 48 bytes. The all-zero fallback is unreachable
    // and fails closed: it never unwraps.
    let ciphertext =
        <[u8; 32 + TAG_LEN]>::try_from(sealed.as_slice()).unwrap_or([0u8; 32 + TAG_LEN]);
    WrappedKey { nonce, ciphertext }
}

/// Reverses [`wrap_key`]; a wrong `kek` or `subject` is [`CryptoError::Authentication`].
pub fn unwrap_key(
    wrapped: &WrappedKey,
    kek: &Secret32,
    subject: &[u8],
) -> Result<Secret32, CryptoError> {
    let plaintext = aead::open(kek, &wrapped.nonce, &wrap_aad(subject), &wrapped.ciphertext)?;
    let key: &[u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Authentication)?;
    Ok(Secret32::from_bytes(*key))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use kb_types::testing::SeededRng;

    fn secret(byte: u8) -> Secret32 {
        Secret32::from_bytes([byte; 32])
    }

    #[test]
    fn prefix_and_length_are_frozen() {
        assert_eq!(WRAP_AAD_PREFIX, b"atomik-kb/v1/wrap");
        assert_eq!(WRAPPED_KEY_LEN, 72);
    }

    #[test]
    fn round_trip() {
        let rng = SeededRng::new(1);
        let wrapped = wrap_key(&secret(7), &secret(9), b"space:1", &rng);
        assert_eq!(
            unwrap_key(&wrapped, &secret(9), b"space:1").unwrap(),
            secret(7)
        );
        let bytes = wrapped.to_bytes();
        assert_eq!(bytes.len(), WRAPPED_KEY_LEN);
        assert_eq!(WrappedKey::from_bytes(&bytes).unwrap(), wrapped);
        assert_ne!(&bytes[NONCE_LEN..NONCE_LEN + 32], &[7u8; 32], "encrypted");
    }

    #[test]
    fn wrong_kek_or_subject_fails() {
        let rng = SeededRng::new(2);
        let wrapped = wrap_key(&secret(7), &secret(9), b"doc:abc", &rng);
        assert_eq!(
            unwrap_key(&wrapped, &secret(8), b"doc:abc"),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            unwrap_key(&wrapped, &secret(9), b"doc:abd"),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            unwrap_key(&wrapped, &secret(9), b""),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn the_aad_is_the_prefix_then_the_subject() {
        let rng = SeededRng::new(3);
        let wrapped = wrap_key(&secret(1), &secret(2), b"subject", &rng);
        let opened = aead::open(
            &secret(2),
            &wrapped.nonce,
            b"atomik-kb/v1/wrapsubject",
            &wrapped.ciphertext,
        )
        .unwrap();
        assert_eq!(opened.as_slice(), &[1u8; 32]);
    }

    #[test]
    fn tampering_any_byte_fails() {
        let rng = SeededRng::new(4);
        let bytes = wrap_key(&secret(1), &secret(2), b"s", &rng).to_bytes();
        for i in 0..bytes.len() {
            let mut tampered = bytes;
            tampered[i] ^= 0x01;
            let wrapped = WrappedKey::from_bytes(&tampered).unwrap();
            assert_eq!(
                unwrap_key(&wrapped, &secret(2), b"s"),
                Err(CryptoError::Authentication),
                "byte {i}"
            );
        }
    }

    #[test]
    fn every_wrap_uses_a_fresh_nonce() {
        let rng = SeededRng::new(5);
        let a = wrap_key(&secret(1), &secret(2), b"s", &rng);
        let b = wrap_key(&secret(1), &secret(2), b"s", &rng);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn from_bytes_requires_the_exact_length() {
        for len in [0, 1, 24, 71, 73, 144] {
            assert!(
                matches!(
                    WrappedKey::from_bytes(&vec![0u8; len]),
                    Err(CryptoError::Encoding(_))
                ),
                "{len} bytes"
            );
        }
        assert!(WrappedKey::from_bytes(&[0u8; WRAPPED_KEY_LEN]).is_ok());
    }
}
