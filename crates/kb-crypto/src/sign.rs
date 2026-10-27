//! Ed25519 signatures with per-context domain separation
//! (`encryption-design.md`: "every message domain-separated by a context prefix").
//!
//! Every signature covers `atomik-kb/v1/` || context || `0x00` || message, so a
//! signature made for one purpose (a device log entry, an envelope of one kind, ...)
//! never verifies for another. Verification is strict: non-canonical encodings,
//! small-order points and non-reduced scalars are refused.

use ed25519_dalek::ed25519::signature::MultipartSigner;

use crate::{CryptoError, LABEL_PREFIX, Secret32};

/// Ed25519 signature length.
pub const SIGNATURE_LEN: usize = 64;

/// Separator between the context and the message in the signed bytes.
const CONTEXT_SEPARATOR: u8 = 0x00;

/// An Ed25519 private key, held by value. Never printed.
#[derive(Clone)]
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    /// Deterministic key from a 32-byte seed (the seed is what the keystore holds).
    #[must_use]
    pub fn from_seed(seed: &Secret32) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(seed.expose()))
    }

    /// The seed this key was made from, for the keystore.
    #[must_use]
    pub fn to_seed(&self) -> Secret32 {
        Secret32::from_bytes(self.0.to_bytes())
    }

    /// The public half.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey(self.0.verifying_key().to_bytes())
    }
}

impl core::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SigningKey(<redacted>)")
    }
}

/// An Ed25519 public key (32 bytes). Public: may be displayed and logged.
///
/// Every value is a canonically encoded point of the prime-order subgroup (other
/// than the identity): [`Self::from_bytes`] refuses anything else, and keys derived
/// from a seed always are.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct VerifyingKey([u8; 32]);

impl VerifyingKey {
    /// Parses 32 bytes. A wrong length, bytes that are not a curve point, a
    /// non-canonical encoding, or a point outside the prime-order subgroup (small
    /// order or a torsion component: keys no honest signer has) are
    /// [`CryptoError::Encoding`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
            CryptoError::Encoding(format!(
                "an Ed25519 public key is 32 bytes, got {}",
                bytes.len()
            ))
        })?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| {
            CryptoError::Encoding("the Ed25519 public key is not a curve point".to_owned())
        })?;
        let point = key.to_edwards();
        if point.compress().to_bytes() != bytes {
            return Err(CryptoError::Encoding(
                "the Ed25519 public key is not canonically encoded".to_owned(),
            ));
        }
        if point.is_small_order() || !point.is_torsion_free() {
            return Err(CryptoError::Encoding(
                "the Ed25519 public key is outside the prime-order subgroup".to_owned(),
            ));
        }
        Ok(Self(bytes))
    }

    /// The raw bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// Lowercase hex, for display and the API.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// A detached Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Signature([u8; SIGNATURE_LEN]);

impl Signature {
    /// Wraps 64 bytes; another length is [`CryptoError::Encoding`]. The contents are
    /// only checked by [`verify`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let bytes: [u8; SIGNATURE_LEN] = bytes.try_into().map_err(|_| {
            CryptoError::Encoding(format!(
                "an Ed25519 signature is {SIGNATURE_LEN} bytes, got {}",
                bytes.len()
            ))
        })?;
        Ok(Self(bytes))
    }

    /// The raw bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; SIGNATURE_LEN] {
        self.0
    }
}

/// Plain Ed25519 (RFC 8032) over the concatenation of `parts`, without our context
/// framing. The RFC known-answer tests run against this.
pub(crate) fn sign_raw(key: &SigningKey, parts: &[&[u8]]) -> Signature {
    match key.0.try_multipart_sign(parts) {
        Ok(signature) => Signature(signature.to_bytes()),
        // ed25519-dalek never fails to sign. Should it ever, the all-zero signature
        // fails closed: its R is a small-order point, which strict verification
        // refuses.
        Err(_) => Signature([0u8; SIGNATURE_LEN]),
    }
}

/// Strict Ed25519 verification (RFC 8032 plus the refusal of small-order `R` and
/// keys, and of non-canonical `R` and `S`), without our context framing.
pub(crate) fn verify_raw(
    key: &VerifyingKey,
    message: &[u8],
    signature: &Signature,
) -> Result<(), CryptoError> {
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&key.0).map_err(|_| CryptoError::Authentication)?;
    let signature = ed25519_dalek::Signature::from_bytes(&signature.0);
    key.verify_strict(message, &signature)
        .map_err(|_| CryptoError::Authentication)
}

/// `atomik-kb/v1/` || `context` || `0x00` || `message`: the bytes actually signed.
fn framed(context: &str, message: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(LABEL_PREFIX.len() + context.len() + 1 + message.len());
    framed.extend_from_slice(LABEL_PREFIX.as_bytes());
    framed.extend_from_slice(context.as_bytes());
    framed.push(CONTEXT_SEPARATOR);
    framed.extend_from_slice(message);
    framed
}

/// Signs `atomik-kb/v1/` || `context` || `0x00` || `message`.
///
/// `context` is a fixed ASCII name such as `device-log` or `envelope/entry`. It
/// must not contain a NUL byte (the separator): `("a\0b", "c")` and `("a", "b\0c")`
/// would frame to the same bytes. Such a context is never signed: the result is
/// the all-zero signature, which never verifies (its `R` has small order).
#[must_use]
pub fn sign(key: &SigningKey, context: &str, message: &[u8]) -> Signature {
    if context.as_bytes().contains(&CONTEXT_SEPARATOR) {
        return Signature([0u8; SIGNATURE_LEN]);
    }
    sign_raw(
        key,
        &[
            LABEL_PREFIX.as_bytes(),
            context.as_bytes(),
            &[CONTEXT_SEPARATOR],
            message,
        ],
    )
}

/// Verifies a signature made by [`sign`] with the same `context`; any failure
/// (wrong key, context or message, a tampered or malleated signature, a context
/// containing a NUL byte) is [`CryptoError::Authentication`].
pub fn verify(
    key: &VerifyingKey,
    context: &str,
    message: &[u8],
    signature: &Signature,
) -> Result<(), CryptoError> {
    if context.as_bytes().contains(&CONTEXT_SEPARATOR) {
        return Err(CryptoError::Authentication);
    }
    verify_raw(key, &framed(context, message), signature)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{hex, kat, unhex, unhex32};

    fn key(byte: u8) -> SigningKey {
        SigningKey::from_seed(&Secret32::from_bytes([byte; 32]))
    }

    /// Small-order points in canonical encoding: the identity, the point of order 2,
    /// the two points of order 4 and two of the points of order 8.
    const SMALL_ORDER: [&str; 6] = [
        "0100000000000000000000000000000000000000000000000000000000000000",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
    ];

    #[test]
    fn matches_rfc8032_section_7_1_tests_1_to_3() {
        let file = kat("kat-ed25519.json");
        let vectors = file["vectors"].as_array().unwrap();
        assert_eq!(vectors.len(), 3);
        for vector in vectors {
            let name = vector["name"].as_str().unwrap();
            let seed = Secret32::from_bytes(unhex32(vector["secret_key_hex"].as_str().unwrap()));
            let public = vector["public_key_hex"].as_str().unwrap();
            let message = unhex(vector["message_hex"].as_str().unwrap());
            let expected = vector["signature_hex"].as_str().unwrap();

            let signing = SigningKey::from_seed(&seed);
            assert_eq!(
                signing.verifying_key().to_hex(),
                public,
                "{name}: public key"
            );
            let parsed = VerifyingKey::from_bytes(&unhex(public)).unwrap();
            assert_eq!(parsed, signing.verifying_key(), "{name}: parsing");

            let signature = sign_raw(&signing, &[message.as_slice()]);
            assert_eq!(hex(&signature.to_bytes()), expected, "{name}: signature");
            verify_raw(&parsed, &message, &signature).unwrap();

            let mut forged = signature.to_bytes();
            forged[0] ^= 1;
            assert_eq!(
                verify_raw(&parsed, &message, &Signature(forged)),
                Err(CryptoError::Authentication),
                "{name}: tampered signature"
            );
        }
    }

    #[test]
    fn signed_bytes_are_prefix_context_nul_message() {
        let signing = key(1);
        let signature = sign(&signing, "device-log", b"entry");
        assert_eq!(
            signature,
            sign_raw(&signing, &[b"atomik-kb/v1/device-log\x00entry".as_slice()]),
            "the framing is exactly prefix || context || 0x00 || message"
        );
        verify_raw(
            &signing.verifying_key(),
            b"atomik-kb/v1/device-log\x00entry",
            &signature,
        )
        .unwrap();
        // Ed25519 is deterministic: same key, context and message, same signature.
        assert_eq!(signature, sign(&signing, "device-log", b"entry"));
    }

    #[test]
    fn round_trip_and_context_separation() {
        let signing = key(2);
        let public = signing.verifying_key();
        let message = b"the same message";
        let a = sign(&signing, "envelope/entry", message);
        let b = sign(&signing, "envelope/head", message);
        verify(&public, "envelope/entry", message, &a).unwrap();
        verify(&public, "envelope/head", message, &b).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            verify(&public, "envelope/head", message, &a),
            Err(CryptoError::Authentication),
            "a signature for one context never verifies for another"
        );
        assert_eq!(
            verify(&public, "envelope/entry", message, &b),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            verify(&public, "", message, &a),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn the_context_message_boundary_is_unambiguous() {
        let signing = key(3);
        let public = signing.verifying_key();
        // "ab" + "c" and "a" + "bc" would collide without the separator.
        let signature = sign(&signing, "ab", b"c");
        assert_eq!(
            verify(&public, "a", b"bc", &signature),
            Err(CryptoError::Authentication)
        );
        // A context with the separator inside is never signed, so its "signature"
        // verifies under no split of the framed bytes.
        let tricky = sign(&signing, "a\0b", b"c");
        assert_eq!(tricky.to_bytes(), [0u8; SIGNATURE_LEN]);
        assert_eq!(
            verify(&public, "a\0b", b"c", &tricky),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            verify(&public, "a", b"b\0c", &tricky),
            Err(CryptoError::Authentication)
        );
        // An honest signature over the same framed bytes is not accepted under a
        // NUL context either.
        let honest = sign(&signing, "a", b"b\0c");
        verify(&public, "a", b"b\0c", &honest).unwrap();
        assert_eq!(
            verify(&public, "a\0b", b"c", &honest),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn the_all_zero_signature_never_verifies() {
        for seed in 0..8u8 {
            let signing = key(seed);
            let zero = Signature([0u8; SIGNATURE_LEN]);
            assert_eq!(
                verify_raw(&signing.verifying_key(), b"", &zero),
                Err(CryptoError::Authentication)
            );
            assert_eq!(
                verify(&signing.verifying_key(), "ctx", b"m", &zero),
                Err(CryptoError::Authentication)
            );
        }
    }

    #[test]
    fn wrong_key_message_or_signature_fails() {
        let signing = key(4);
        let signature = sign(&signing, "ctx", b"message");
        assert_eq!(
            verify(&key(5).verifying_key(), "ctx", b"message", &signature),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            verify(&signing.verifying_key(), "ctx", b"messagf", &signature),
            Err(CryptoError::Authentication)
        );
        for i in 0..SIGNATURE_LEN {
            let mut bytes = signature.to_bytes();
            bytes[i] ^= 0x10;
            assert_eq!(
                verify(
                    &signing.verifying_key(),
                    "ctx",
                    b"message",
                    &Signature(bytes)
                ),
                Err(CryptoError::Authentication),
                "byte {i}"
            );
        }
    }

    #[test]
    fn a_malleated_signature_is_refused() {
        // S + L (the group order) verifies under lax rules; strict verification
        // refuses the non-reduced scalar.
        const L: [u8; 32] = [
            0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9,
            0xde, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x10,
        ];
        let signing = key(6);
        let mut bytes = sign(&signing, "ctx", b"m").to_bytes();
        let mut carry = 0u16;
        for (s, l) in bytes[32..].iter_mut().zip(L) {
            let sum = u16::from(*s) + u16::from(l) + carry;
            *s = (sum & 0xff) as u8;
            carry = sum >> 8;
        }
        assert_eq!(
            verify(&signing.verifying_key(), "ctx", b"m", &Signature(bytes)),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn verifying_key_parsing_is_strict() {
        for len in [0, 31, 33, 64] {
            assert!(matches!(
                VerifyingKey::from_bytes(&vec![1u8; len]),
                Err(CryptoError::Encoding(_))
            ));
        }
        for encoded in SMALL_ORDER {
            let bytes = unhex(encoded);
            // The points really are on the curve (only the strict checks reject them).
            assert!(ed25519_dalek::VerifyingKey::from_bytes(&unhex32(encoded)).is_ok());
            match VerifyingKey::from_bytes(&bytes) {
                Err(CryptoError::Encoding(message)) => {
                    assert!(message.contains("subgroup"), "{encoded}: {message}");
                }
                other => panic!("{encoded}: {other:?}"),
            }
        }
        // A valid key plus a point of order 8: on the curve, but with torsion.
        let honest = ed25519_dalek::VerifyingKey::from_bytes(&key(7).verifying_key().to_bytes())
            .unwrap()
            .to_edwards();
        let torsion = ed25519_dalek::VerifyingKey::from_bytes(&unhex32(SMALL_ORDER[4]))
            .unwrap()
            .to_edwards();
        let mixed = (honest + torsion).compress().to_bytes();
        assert!(matches!(
            VerifyingKey::from_bytes(&mixed),
            Err(CryptoError::Encoding(m)) if m.contains("subgroup")
        ));
        // y = p + 1 is the identity again, encoded non-canonically.
        let mut non_canonical = [0xffu8; 32];
        non_canonical[0] = 0xee;
        non_canonical[31] = 0x7f;
        assert!(matches!(
            VerifyingKey::from_bytes(&non_canonical),
            Err(CryptoError::Encoding(_))
        ));
        // An honest key round-trips.
        let public = key(8).verifying_key();
        assert_eq!(
            VerifyingKey::from_bytes(&public.to_bytes()).unwrap(),
            public
        );
    }

    #[test]
    fn signature_parsing_checks_the_length() {
        for len in [0, 63, 65] {
            assert!(matches!(
                Signature::from_bytes(&vec![0u8; len]),
                Err(CryptoError::Encoding(_))
            ));
        }
        let signature = sign(&key(9), "c", b"m");
        assert_eq!(
            Signature::from_bytes(&signature.to_bytes()).unwrap(),
            signature
        );
    }

    #[test]
    fn seeds_round_trip_and_debug_is_redacted() {
        let seed = Secret32::from_bytes([0xab; 32]);
        let signing = SigningKey::from_seed(&seed);
        assert_eq!(signing.to_seed(), seed);
        let text = format!("{signing:?}");
        assert_eq!(text, "SigningKey(<redacted>)");
        assert!(!text.contains("ab") && !text.contains("171"));
        assert_eq!(key(1).verifying_key().to_hex().len(), 64);
    }
}
