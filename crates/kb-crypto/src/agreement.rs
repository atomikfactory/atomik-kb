//! X25519 key agreement and a libsodium-shaped sealed box
//! (`encryption-design.md`, "Key agreement / sealing"). Used by pairing (M7) and,
//! later, by sharing; implemented and tested in Milestone 4 so the suite is complete.
//!
//! Sealed box layout: `ephemeral_public (32) || nonce (24) || ciphertext || tag (16)`.
//! The XChaCha20-Poly1305 key is `HKDF(shared, pairing-channel, info = ephemeral
//! public || recipient public)`, the AAD `atomik-kb/v1/sealed || context`.

use kb_types::Rng;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::aead::{self, NONCE_LEN, Nonce24, TAG_LEN};
use crate::{CryptoError, Label, Secret32, derive_key};

/// Length of an X25519 public key.
pub const AGREEMENT_PUBLIC_LEN: usize = 32;
/// AAD prefix of a sealed box: `atomik-kb/v1/sealed` || context.
pub const SEALED_AAD_PREFIX: &[u8] = b"atomik-kb/v1/sealed";
/// Bytes a sealed box adds to its plaintext: ephemeral public key, nonce and tag.
pub const SEALED_OVERHEAD: usize = AGREEMENT_PUBLIC_LEN + NONCE_LEN + TAG_LEN;

/// Scalar used to detect low-order points. Any value works: X25519 clamps it to a
/// multiple of 8 below 8 * l, so the product with a point is zero exactly when the
/// order of the point divides 8.
const LOW_ORDER_PROBE: [u8; 32] = [0x01; 32];

/// An X25519 private key. Never printed.
#[derive(Clone)]
pub struct AgreementSecret(x25519_dalek::StaticSecret);

impl AgreementSecret {
    /// Deterministic key from a 32-byte seed (what the keystore holds).
    #[must_use]
    pub fn from_seed(seed: &Secret32) -> Self {
        Self(x25519_dalek::StaticSecret::from(*seed.expose()))
    }

    /// The seed, for the keystore.
    #[must_use]
    pub fn to_seed(&self) -> Secret32 {
        Secret32::from_bytes(self.0.to_bytes())
    }

    /// The public half.
    #[must_use]
    pub fn public(&self) -> AgreementPublic {
        AgreementPublic(x25519_dalek::PublicKey::from(&self.0).to_bytes())
    }
}

impl core::fmt::Debug for AgreementSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("AgreementSecret(<redacted>)")
    }
}

/// An X25519 public key (32 bytes).
///
/// Every value is a canonical encoding of a point whose order does not divide 8:
/// [`Self::from_bytes`] refuses anything else, and keys computed from a secret
/// always are. A shared secret with such a key is therefore never all-zero, which
/// is what lets [`seal_to`] work without a failure case.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct AgreementPublic([u8; AGREEMENT_PUBLIC_LEN]);

impl AgreementPublic {
    /// Parses 32 bytes. A wrong length, a non-canonical encoding (the top bit set or
    /// a value of at least 2^255 - 19) or a low-order point (one that would make
    /// every shared secret zero) is [`CryptoError::Encoding`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let bytes: [u8; AGREEMENT_PUBLIC_LEN] = bytes.try_into().map_err(|_| {
            CryptoError::Encoding(format!(
                "an X25519 public key is {AGREEMENT_PUBLIC_LEN} bytes, got {}",
                bytes.len()
            ))
        })?;
        if !is_canonical(&bytes) {
            return Err(CryptoError::Encoding(
                "the X25519 public key is not canonically encoded".to_owned(),
            ));
        }
        if is_low_order(&bytes) {
            return Err(CryptoError::Encoding(
                "the X25519 public key has low order".to_owned(),
            ));
        }
        Ok(Self(bytes))
    }

    /// The raw bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; AGREEMENT_PUBLIC_LEN] {
        self.0
    }
}

/// A little-endian field element below p = 2^255 - 19 with the unused top bit clear.
fn is_canonical(u: &[u8; 32]) -> bool {
    if u[31] & 0x80 != 0 {
        return false;
    }
    // The only values in [p, 2^255) are 0x7fff..ffed through 0x7fff..ffff.
    let at_least_p = u[31] == 0x7f && u[1..31].iter().all(|&b| b == 0xff) && u[0] >= 0xed;
    !at_least_p
}

/// `true` for the u-coordinates of points of order 1, 2, 4 or 8 (on the curve or
/// its twist): exactly those give an all-zero result for every scalar.
fn is_low_order(u: &[u8; 32]) -> bool {
    let product = x25519_dalek::x25519(LOW_ORDER_PROBE, *u);
    product[..].ct_eq(&[0u8; 32][..]).into()
}

/// Raw Diffie-Hellman output as key material (callers derive purpose keys with HKDF).
/// An all-zero result (low-order peer point) is [`CryptoError::Authentication`].
pub fn shared_secret(
    secret: &AgreementSecret,
    peer: &AgreementPublic,
) -> Result<Secret32, CryptoError> {
    let shared = secret
        .0
        .diffie_hellman(&x25519_dalek::PublicKey::from(peer.0));
    if bool::from(shared.as_bytes()[..].ct_eq(&[0u8; 32][..])) {
        return Err(CryptoError::Authentication);
    }
    Ok(Secret32::from_bytes(*shared.as_bytes()))
}

/// The box key: `HKDF(shared, pairing-channel, info = ephemeral || recipient)`.
fn box_key(
    shared: &Secret32,
    ephemeral: &AgreementPublic,
    recipient: &AgreementPublic,
) -> Secret32 {
    let mut info = [0u8; 2 * AGREEMENT_PUBLIC_LEN];
    let (first, second) = info.split_at_mut(AGREEMENT_PUBLIC_LEN);
    first.copy_from_slice(&ephemeral.0);
    second.copy_from_slice(&recipient.0);
    derive_key(shared, Label::PAIRING_CHANNEL, &info)
}

/// `SEALED_AAD_PREFIX || context`.
fn sealed_aad(context: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(SEALED_AAD_PREFIX.len() + context.len());
    aad.extend_from_slice(SEALED_AAD_PREFIX);
    aad.extend_from_slice(context.as_bytes());
    aad
}

/// Sealed box: ephemeral X25519 key, HKDF(shared, `pairing-channel`, info = ephemeral
/// public || recipient public) as the XChaCha key, AAD = `atomik-kb/v1/sealed` ||
/// context; output `ephemeral_public || nonce || ciphertext`.
///
/// Every call draws a fresh ephemeral key and nonce from `rng`, so sealing the same
/// plaintext twice gives unrelated boxes. Only the recipient can open it; the box
/// does not say who sealed it (authenticate the sender separately when it matters).
#[must_use]
pub fn seal_to(
    recipient: &AgreementPublic,
    context: &str,
    plaintext: &[u8],
    rng: &dyn Rng,
) -> Vec<u8> {
    let ephemeral = AgreementSecret::from_seed(&Secret32::generate(rng));
    let ephemeral_public = ephemeral.public();
    // Unreachable for a valid `AgreementPublic` (see its invariant); refusing to
    // seal is the closed failure: never encrypt under a key anyone could compute.
    let Ok(shared) = shared_secret(&ephemeral, recipient) else {
        return Vec::new();
    };
    let key = box_key(&shared, &ephemeral_public, recipient);
    let nonce = Nonce24::random(rng);
    let ciphertext = aead::seal(&key, &nonce, &sealed_aad(context), plaintext);
    let mut out = Vec::with_capacity(AGREEMENT_PUBLIC_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&ephemeral_public.0);
    out.extend_from_slice(nonce.as_bytes());
    out.extend_from_slice(&ciphertext);
    out
}

/// Opens a sealed box addressed to `secret`'s public key. Input shorter than
/// [`SEALED_OVERHEAD`] is [`CryptoError::Encoding`]; everything else that does not
/// open (another recipient, another context, an invalid ephemeral key, tampering)
/// is [`CryptoError::Authentication`].
pub fn open_sealed(
    secret: &AgreementSecret,
    context: &str,
    sealed: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let too_short = || {
        CryptoError::Encoding(format!(
            "a sealed box is at least {SEALED_OVERHEAD} bytes, got {}",
            sealed.len()
        ))
    };
    if sealed.len() < SEALED_OVERHEAD {
        return Err(too_short());
    }
    let (ephemeral, rest) = sealed
        .split_first_chunk::<AGREEMENT_PUBLIC_LEN>()
        .ok_or_else(too_short)?;
    let (nonce, ciphertext) = rest
        .split_first_chunk::<NONCE_LEN>()
        .ok_or_else(too_short)?;
    let ephemeral =
        AgreementPublic::from_bytes(ephemeral).map_err(|_| CryptoError::Authentication)?;
    let shared = shared_secret(secret, &ephemeral)?;
    let key = box_key(&shared, &ephemeral, &secret.public());
    aead::open(
        &key,
        &Nonce24::from_bytes(*nonce),
        &sealed_aad(context),
        ciphertext,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{hex, kat, unhex, unhex32};
    use kb_types::testing::SeededRng;

    fn secret(byte: u8) -> AgreementSecret {
        AgreementSecret::from_seed(&Secret32::from_bytes([byte; 32]))
    }

    /// u-coordinates of low-order points, canonical encodings (libsodium blocklist).
    const LOW_ORDER_CANONICAL: [&str; 5] = [
        // 0 (order 4)
        "0000000000000000000000000000000000000000000000000000000000000000",
        // 1 (order 1)
        "0100000000000000000000000000000000000000000000000000000000000000",
        // order 8
        "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
        // order 8
        "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
        // p - 1 (order 2)
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ];

    #[test]
    fn matches_rfc7748_section_6_1() {
        let file = kat("kat-x25519.json");
        let vectors = file["vectors"].as_array().unwrap();
        assert_eq!(vectors.len(), 1);
        for vector in vectors {
            let alice = AgreementSecret::from_seed(&Secret32::from_bytes(unhex32(
                vector["alice_private_hex"].as_str().unwrap(),
            )));
            let bob = AgreementSecret::from_seed(&Secret32::from_bytes(unhex32(
                vector["bob_private_hex"].as_str().unwrap(),
            )));
            let alice_public = vector["alice_public_hex"].as_str().unwrap();
            let bob_public = vector["bob_public_hex"].as_str().unwrap();
            let shared = vector["shared_hex"].as_str().unwrap();

            assert_eq!(hex(&alice.public().to_bytes()), alice_public);
            assert_eq!(hex(&bob.public().to_bytes()), bob_public);
            let bob_parsed = AgreementPublic::from_bytes(&unhex(bob_public)).unwrap();
            let alice_parsed = AgreementPublic::from_bytes(&unhex(alice_public)).unwrap();
            assert_eq!(
                hex(shared_secret(&alice, &bob_parsed).unwrap().expose()),
                shared
            );
            assert_eq!(
                hex(shared_secret(&bob, &alice_parsed).unwrap().expose()),
                shared
            );
        }
    }

    #[test]
    fn low_order_and_non_canonical_points_are_refused() {
        for encoded in LOW_ORDER_CANONICAL {
            let u = unhex32(encoded);
            // Independent of our parser: the raw function gives zero for any scalar.
            assert_eq!(x25519_dalek::x25519([0x5a; 32], u), [0u8; 32], "{encoded}");
            assert_eq!(
                AgreementPublic::from_bytes(&u),
                Err(CryptoError::Encoding(
                    "the X25519 public key has low order".to_owned()
                )),
                "{encoded}"
            );
            // The same value with the unused top bit set is non-canonical.
            let mut high = u;
            high[31] |= 0x80;
            assert!(matches!(
                AgreementPublic::from_bytes(&high),
                Err(CryptoError::Encoding(m)) if m.contains("canonical")
            ));
        }
        // p and p + 1: zero and one, encoded non-canonically.
        for first in [0xed_u8, 0xee, 0xff] {
            let mut u = [0xff_u8; 32];
            u[0] = first;
            u[31] = 0x7f;
            assert!(matches!(
                AgreementPublic::from_bytes(&u),
                Err(CryptoError::Encoding(m)) if m.contains("canonical")
            ));
        }
        // p - 2: canonical (p - 1 is the largest canonical value, of order 2) and a
        // regular point.
        let mut largest = [0xff_u8; 32];
        largest[0] = 0xec - 1;
        largest[31] = 0x7f;
        assert!(AgreementPublic::from_bytes(&largest).is_ok());
        for len in [0, 31, 33] {
            assert!(matches!(
                AgreementPublic::from_bytes(&vec![9u8; len]),
                Err(CryptoError::Encoding(_))
            ));
        }
        let public = secret(1).public();
        assert_eq!(
            AgreementPublic::from_bytes(&public.to_bytes()).unwrap(),
            public
        );
    }

    #[test]
    fn an_all_zero_shared_secret_is_refused() {
        // Not constructible through `from_bytes`; built directly to test the guard.
        for encoded in LOW_ORDER_CANONICAL {
            let peer = AgreementPublic(unhex32(encoded));
            assert_eq!(
                shared_secret(&secret(3), &peer),
                Err(CryptoError::Authentication),
                "{encoded}"
            );
            let rng = SeededRng::new(1);
            assert!(
                seal_to(&peer, "ctx", b"never under a public key", &rng).is_empty(),
                "sealing to a low-order point fails closed"
            );
        }
    }

    #[test]
    fn shared_secret_is_symmetric() {
        let (a, b) = (secret(1), secret(2));
        assert_eq!(
            shared_secret(&a, &b.public()).unwrap(),
            shared_secret(&b, &a.public()).unwrap()
        );
        assert_ne!(
            shared_secret(&a, &b.public()).unwrap(),
            shared_secret(&a, &secret(3).public()).unwrap()
        );
    }

    #[test]
    fn sealed_box_round_trip_and_layout() {
        let rng = SeededRng::new(11);
        let recipient = secret(7);
        let plaintext = b"space key for the new device";
        let sealed = seal_to(&recipient.public(), "pairing/bundle", plaintext, &rng);
        assert_eq!(sealed.len(), SEALED_OVERHEAD + plaintext.len());
        let ephemeral = AgreementPublic::from_bytes(&sealed[..32]).unwrap();
        assert_ne!(ephemeral, recipient.public());

        let opened = open_sealed(&recipient, "pairing/bundle", &sealed).unwrap();
        assert_eq!(opened.as_slice(), plaintext);

        // The documented construction, spelled out.
        let shared = shared_secret(&recipient, &ephemeral).unwrap();
        let mut info = sealed[..32].to_vec();
        info.extend_from_slice(&recipient.public().to_bytes());
        let key = derive_key(&shared, Label::PAIRING_CHANNEL, &info);
        let nonce = Nonce24::from_bytes(sealed[32..56].try_into().unwrap());
        let manual = aead::open(
            &key,
            &nonce,
            b"atomik-kb/v1/sealedpairing/bundle",
            &sealed[56..],
        );
        assert_eq!(manual.unwrap().as_slice(), plaintext);
    }

    #[test]
    fn every_seal_is_fresh() {
        let rng = SeededRng::new(12);
        let recipient = secret(7).public();
        let a = seal_to(&recipient, "c", b"same", &rng);
        let b = seal_to(&recipient, "c", b"same", &rng);
        assert_ne!(a[..32], b[..32], "fresh ephemeral key");
        assert_ne!(a[32..56], b[32..56], "fresh nonce");
    }

    #[test]
    fn wrong_recipient_or_context_fails() {
        let rng = SeededRng::new(13);
        let sealed = seal_to(&secret(7).public(), "ctx", b"secret", &rng);
        assert_eq!(
            open_sealed(&secret(8), "ctx", &sealed),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            open_sealed(&secret(7), "ctx2", &sealed),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn tampering_any_region_fails() {
        let rng = SeededRng::new(14);
        let recipient = secret(9);
        let sealed = seal_to(&recipient.public(), "ctx", b"payload", &rng);
        for i in 0..sealed.len() {
            let mut tampered = sealed.clone();
            tampered[i] ^= 0x04;
            assert!(
                open_sealed(&recipient, "ctx", &tampered).is_err(),
                "byte {i}"
            );
        }
        for len in 0..SEALED_OVERHEAD {
            assert!(matches!(
                open_sealed(&recipient, "ctx", &sealed[..len]),
                Err(CryptoError::Encoding(_))
            ));
        }
        let mut low_order = sealed.clone();
        low_order[..32].copy_from_slice(&[0u8; 32]);
        assert_eq!(
            open_sealed(&recipient, "ctx", &low_order),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn empty_plaintext_seals() {
        let rng = SeededRng::new(15);
        let sealed = seal_to(&secret(1).public(), "", b"", &rng);
        assert_eq!(sealed.len(), SEALED_OVERHEAD);
        assert!(open_sealed(&secret(1), "", &sealed).unwrap().is_empty());
    }

    #[test]
    fn seeds_round_trip_and_debug_is_redacted() {
        let seed = Secret32::from_bytes([0xcd; 32]);
        let agreement = AgreementSecret::from_seed(&seed);
        assert_eq!(agreement.to_seed(), seed);
        let text = format!("{agreement:?}");
        assert_eq!(text, "AgreementSecret(<redacted>)");
        assert!(!text.contains("cd") && !text.contains("205"));
    }

    #[test]
    fn prefix_is_frozen() {
        assert_eq!(SEALED_AAD_PREFIX, b"atomik-kb/v1/sealed");
        assert_eq!(SEALED_OVERHEAD, 72);
    }
}
