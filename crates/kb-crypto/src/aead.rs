//! XChaCha20-Poly1305 (`docs/security/encryption-design.md`, suite v1): 256-bit
//! key, 192-bit random nonce, 128-bit Poly1305 tag appended to the ciphertext
//! (draft-irtf-cfrg-xchacha-03). The only AEAD of suite v1; every other module
//! (wrapping, envelopes, sealed boxes, streams) is built on [`seal`] and [`open`].

use chacha20poly1305::aead::AeadInOut;
use chacha20poly1305::{Key, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use kb_types::Rng;
use zeroize::{Zeroize, Zeroizing};

use crate::{CryptoError, Secret32};

/// XChaCha20 nonce length.
pub const NONCE_LEN: usize = 24;
/// Poly1305 tag length.
pub const TAG_LEN: usize = 16;

/// A 192-bit nonce. Random nonces are safe at this size without any coordination
/// between devices (`encryption-design.md`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Nonce24([u8; NONCE_LEN]);

impl Nonce24 {
    /// Wraps nonce bytes read from storage.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; NONCE_LEN]) -> Self {
        Self(bytes)
    }

    /// A fresh random nonce from the injected CSPRNG.
    #[must_use]
    pub fn random(rng: &dyn Rng) -> Self {
        let mut bytes = [0u8; NONCE_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; NONCE_LEN] {
        &self.0
    }
}

/// The cipher keyed with `key`; the crate feature `zeroize` wipes its key copy on
/// drop. The key is borrowed in place, never copied into a temporary of ours.
fn cipher(key: &Secret32) -> XChaCha20Poly1305 {
    let key: &Key = key.expose().into();
    XChaCha20Poly1305::new(key)
}

/// [`seal`] reporting its one possible failure: a plaintext beyond the XChaCha20
/// block counter (2^32 blocks of 64 bytes, 256 GiB) is [`CryptoError::Encoding`].
pub(crate) fn try_seal(
    key: &Secret32,
    nonce: &Nonce24,
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    // Exact capacity: the tag is appended without a reallocation, so no copy of the
    // plaintext is left behind in a freed buffer.
    let mut out = Vec::with_capacity(plaintext.len().saturating_add(TAG_LEN));
    out.extend_from_slice(plaintext);
    let xnonce: &XNonce = nonce.as_bytes().into();
    match cipher(key).encrypt_inout_detached(xnonce, aad, out.as_mut_slice().into()) {
        Ok(tag) => {
            out.extend_from_slice(&tag);
            Ok(out)
        }
        Err(_) => {
            // Nothing was encrypted: the buffer still holds the plaintext.
            out.zeroize();
            Err(CryptoError::Encoding(
                "plaintext too long for XChaCha20-Poly1305".to_owned(),
            ))
        }
    }
}

/// Encrypts `plaintext` under `key` with `aad` authenticated; returns
/// `ciphertext || tag` (`plaintext.len() + TAG_LEN` bytes).
///
/// A plaintext beyond the limit of the cipher (256 GiB: unreachable in practice,
/// impossible on wasm32) yields an empty vector, which [`open`] always rejects; the
/// failure is closed, never a ciphertext under a different construction.
#[must_use]
pub fn seal(key: &Secret32, nonce: &Nonce24, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    try_seal(key, nonce, aad, plaintext).unwrap_or_default()
}

/// Decrypts and authenticates `ciphertext || tag`. Any failure (wrong key, wrong
/// nonce, wrong AAD, tampering, short input) is [`CryptoError::Authentication`].
/// Nothing is decrypted before the tag has been verified.
pub fn open(
    key: &Secret32,
    nonce: &Nonce24,
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let body_len = ciphertext
        .len()
        .checked_sub(TAG_LEN)
        .ok_or(CryptoError::Authentication)?;
    let (body, tag) = ciphertext
        .split_at_checked(body_len)
        .ok_or(CryptoError::Authentication)?;
    let tag = Tag::try_from(tag).map_err(|_| CryptoError::Authentication)?;
    let mut plaintext = Zeroizing::new(body.to_vec());
    let xnonce: &XNonce = nonce.as_bytes().into();
    cipher(key)
        .decrypt_inout_detached(xnonce, aad, plaintext.as_mut_slice().into(), &tag)
        .map_err(|_| CryptoError::Authentication)?;
    Ok(plaintext)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{hex, kat, unhex};
    use kb_types::testing::SeededRng;

    fn key(byte: u8) -> Secret32 {
        Secret32::from_bytes([byte; 32])
    }

    #[test]
    fn matches_draft_irtf_cfrg_xchacha_03_a_3_1() {
        let file = kat("kat-xchacha20poly1305.json");
        let vectors = file["vectors"].as_array().unwrap();
        assert!(!vectors.is_empty());
        for vector in vectors {
            let key = Secret32::from_bytes(
                unhex(vector["key_hex"].as_str().unwrap())
                    .try_into()
                    .unwrap(),
            );
            let nonce = Nonce24::from_bytes(
                unhex(vector["nonce_hex"].as_str().unwrap())
                    .try_into()
                    .unwrap(),
            );
            let aad = unhex(vector["aad_hex"].as_str().unwrap());
            let plaintext = unhex(vector["plaintext_hex"].as_str().unwrap());
            let expected = format!(
                "{}{}",
                vector["ciphertext_hex"].as_str().unwrap(),
                vector["tag_hex"].as_str().unwrap()
            );

            let sealed = seal(&key, &nonce, &aad, &plaintext);
            assert_eq!(hex(&sealed), expected);
            let opened = open(&key, &nonce, &aad, &sealed).unwrap();
            assert_eq!(opened.as_slice(), plaintext.as_slice());
        }
    }

    #[test]
    fn round_trip_and_length() {
        let rng = SeededRng::new(7);
        for len in [0usize, 1, 15, 16, 17, 63, 64, 65, 1000] {
            let plaintext: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let nonce = Nonce24::random(&rng);
            let sealed = seal(&key(1), &nonce, b"aad", &plaintext);
            assert_eq!(sealed.len(), len + TAG_LEN);
            if len > 0 {
                assert_ne!(&sealed[..len], plaintext.as_slice(), "encrypted");
            }
            let opened = open(&key(1), &nonce, b"aad", &sealed).unwrap();
            assert_eq!(opened.as_slice(), plaintext.as_slice());
        }
    }

    #[test]
    fn wrong_key_nonce_or_aad_fails() {
        let nonce = Nonce24::from_bytes([3; NONCE_LEN]);
        let sealed = seal(&key(1), &nonce, b"aad", b"secret");
        assert_eq!(
            open(&key(2), &nonce, b"aad", &sealed),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            open(
                &key(1),
                &Nonce24::from_bytes([4; NONCE_LEN]),
                b"aad",
                &sealed
            ),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            open(&key(1), &nonce, b"aaD", &sealed),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            open(&key(1), &nonce, b"", &sealed),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn every_flipped_bit_fails() {
        let nonce = Nonce24::from_bytes([9; NONCE_LEN]);
        let sealed = seal(&key(5), &nonce, b"header", b"twenty bytes of data");
        for i in 0..sealed.len() {
            for bit in 0..8 {
                let mut tampered = sealed.clone();
                tampered[i] ^= 1 << bit;
                assert_eq!(
                    open(&key(5), &nonce, b"header", &tampered),
                    Err(CryptoError::Authentication),
                    "byte {i} bit {bit}"
                );
            }
        }
    }

    #[test]
    fn short_or_cut_input_is_authentication() {
        let nonce = Nonce24::from_bytes([1; NONCE_LEN]);
        let sealed = seal(&key(1), &nonce, b"", b"some plaintext");
        for len in 0..sealed.len() {
            assert_eq!(
                open(&key(1), &nonce, b"", &sealed[..len]),
                Err(CryptoError::Authentication),
                "prefix of {len} bytes"
            );
        }
        let mut extended = sealed.clone();
        extended.push(0);
        assert_eq!(
            open(&key(1), &nonce, b"", &extended),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn random_nonces_come_from_the_injected_rng() {
        let a = Nonce24::random(&SeededRng::new(1));
        let b = Nonce24::random(&SeededRng::new(1));
        let rng = SeededRng::new(2);
        let c = Nonce24::random(&rng);
        let d = Nonce24::random(&rng);
        assert_eq!(a, b, "same seed, same nonce");
        assert_ne!(c, d, "consecutive nonces differ");
        assert_eq!(Nonce24::from_bytes(*c.as_bytes()), c);
    }
}
