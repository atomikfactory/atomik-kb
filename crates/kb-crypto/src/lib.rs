//! Cryptographic primitives for atomik-kb (ADR-0013, `docs/security/encryption-design.md`).
//!
//! Milestone 1: secret newtypes, HKDF-SHA256 derivation with domain labels and
//! BLAKE3 hashing / key check values. Milestone 4: XChaCha20-Poly1305 ([`aead`]),
//! key wrapping ([`wrap`]), Ed25519 ([`sign`](mod@sign)), X25519 and sealed boxes
//! ([`agreement`]), Argon2id passphrase wrapping ([`passphrase`]), envelope v1
//! ([`envelope`]), size-bucket padding ([`padding`]), chunked streaming AEAD
//! ([`stream`]) and the recovery kit encodings ([`kit`]).
//!
//! Rules: pure Rust only; every derivation uses a label from [`Label`]; key material
//! lives in [`Secret32`], which zeroises on drop and never prints its bytes.

#![forbid(unsafe_code)]

pub mod aead;
pub mod agreement;
mod codec;
pub mod envelope;
pub mod kit;
pub mod padding;
pub mod passphrase;
pub mod sign;
pub mod stream;
#[cfg(test)]
mod test_support;
pub mod wrap;

pub use aead::{NONCE_LEN, Nonce24, TAG_LEN, open as aead_open, seal as aead_seal};
pub use agreement::{AgreementPublic, AgreementSecret, open_sealed, seal_to, shared_secret};
pub use envelope::{
    ENVELOPE_VERSION, Envelope, EnvelopeKind, SUITE_ID, open_envelope, seal_envelope,
};
pub use padding::{bucket_size, pad_to_bucket, unpad};
pub use passphrase::{
    Argon2Params, PassphraseWrapped, unwrap_with_passphrase, wrap_with_passphrase,
};
pub use sign::{SIGNATURE_LEN, Signature, SigningKey, VerifyingKey, sign, verify};
pub use stream::{CHUNK_OVERHEAD, CHUNK_SIZE, StreamOpener, StreamSealer, open_chunk_at};
pub use wrap::{WRAPPED_KEY_LEN, WrappedKey, unwrap_key, wrap_key};

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Prefix shared by every HKDF label: `atomik-kb/v1/<purpose>`.
pub const LABEL_PREFIX: &str = "atomik-kb/v1/";

/// HKDF salt, constant for suite v1.
pub const HKDF_SALT: &[u8] = b"atomik-kb/v1";

/// Domain separator in front of the key bytes in a key check value.
const KCV_DOMAIN: &[u8] = b"kcv";

/// Length in bytes of every key [`derive_key`] returns.
const DERIVED_KEY_LEN: usize = 32;

/// Failures of the suite's constructions. Messages never carry key or plaintext bytes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    /// Decryption or signature verification failed (wrong key, tampering).
    #[error("authentication failed")]
    Authentication,
    /// Malformed bytes (length, version, kind, encoding).
    #[error("malformed data: {0}")]
    Encoding(String),
    /// Argon2id parameters below the accepted minimum.
    #[error("parameters below the accepted minimum")]
    WeakParameters,
    /// A recovery kit that does not decode.
    #[error("invalid recovery kit: {0}")]
    InvalidKit(String),
}

/// A domain-separation label for key derivation. Only the constants below exist;
/// adding one means adding it to `docs/security/encryption-design.md` too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(&'static str);

impl Label {
    /// Local SQLCipher/sqlite3mc database key; info = install id bytes.
    pub const LOCAL_DB: Label = Label("local-db");
    /// Key-encryption key wrapping space keys.
    pub const SPACE_KEK: Label = Label("space-kek");
    /// Account document key.
    pub const ACCOUNT_DOC: Label = Label("account-doc");
    /// Keyring record encryption.
    pub const KEYRING: Label = Label("keyring");
    /// Keyring record authentication.
    pub const KEYRING_AUTH: Label = Label("keyring-auth");
    /// Wraps the root key under the recovery key.
    pub const RECOVERY_WRAP: Label = Label("recovery-wrap");
    /// Server lookup id for the recovery blob.
    pub const RECOVERY_ID: Label = Label("recovery-id");
    /// Recovery signing key seed.
    pub const RECOVERY_SIGN: Label = Label("recovery-sign");
    /// Pairing short authentication string.
    pub const PAIRING_SAS: Label = Label("pairing-sas");
    /// Pairing channel key.
    pub const PAIRING_CHANNEL: Label = Label("pairing-channel");
    /// Backup payload key; info = backup id.
    pub const BACKUP: Label = Label("backup");
    /// Device names in the device log.
    pub const DEVICE_NAME: Label = Label("device-name");
    /// Checksum domain of the recovery kit code form (not a key).
    pub const RECOVERY_KIT: Label = Label("recovery-kit");
    /// Key of the private fields of membership entries; ikm = a space key, info = the feed id
    /// (M13, ADR-0043).
    pub const MEMBERSHIP_META: Label = Label("membership-meta");
    /// Transport key of the keyring entries of a shared feed; ikm = a space key, info = the feed id
    /// (M13, ADR-0044).
    pub const SPACE_KEYRING: Label = Label("space-keyring");
    /// The id of an invite; ikm = the invite secret `s`, first 16 bytes (M13, ADR-0046).
    pub const INVITE_ID: Label = Label("invite-id");
    /// The Ed25519 seed of an invite's signing key; ikm = `s` (M13, ADR-0046).
    pub const INVITE_SIGN: Label = Label("invite-sign");
    /// Key of a link invite's sealed bundle; ikm = `s` (M13, ADR-0046).
    pub const INVITE_BUNDLE: Label = Label("invite-bundle");
    /// Key of the presence frames of a shared feed; ikm = a space key, info = the feed id (M13 slice 3,
    /// ADR-0050). Spelled `kb_types::sharing::PRESENCE_LABEL`.
    pub const PRESENCE: Label = Label("presence");
    /// The id of a view link; ikm = the link key `K_link`, no info, first 16 bytes (M13 slice 4, ADR-0051).
    /// Spelled `kb_types::sharing::LINK_ID_LABEL`.
    pub const LINK_ID: Label = Label("link-id");
    /// Key of a view link's publication; ikm = `K_link`, no info (M13 slice 4, ADR-0051). Spelled
    /// `kb_types::sharing::PUBLICATION_LABEL`.
    pub const PUBLICATION: Label = Label("publication");

    /// The purpose part of the label (without the prefix).
    #[must_use]
    pub const fn purpose(self) -> &'static str {
        self.0
    }
}

/// 32 bytes of key material that are zeroised on drop and never displayed.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Secret32([u8; 32]);

impl Secret32 {
    /// Wraps existing key bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Fresh random key material from the injected CSPRNG.
    #[must_use]
    pub fn generate(rng: &dyn kb_types::Rng) -> Self {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Borrow the raw bytes. Keep the borrow short; never log it.
    #[must_use]
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Debug for Secret32 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Secret32(<redacted>)")
    }
}

/// HKDF-SHA256: `okm = HKDF(salt = HKDF_SALT, ikm, info = LABEL_PREFIX || label || 0x00 || info)`.
///
/// The 32-byte output is a fresh key for exactly one purpose: the same `ikm` with a
/// different `label` or `info` gives an unrelated key. The `0x00` separator keeps the
/// label/info boundary unambiguous (label `keyring` + info `-auth` differs from label
/// `keyring-auth` + empty info).
///
/// Key hygiene is best effort: the output buffer is wiped, but the intermediate HMAC
/// state inside the `hkdf` crate is not under our control.
#[must_use]
pub fn derive_key(ikm: &Secret32, label: Label, info: &[u8]) -> Secret32 {
    let mut okm = Zeroizing::new([0u8; DERIVED_KEY_LEN]);
    // The info components are fed in sequence, which equals their concatenation
    // (`LABEL_PREFIX || purpose || 0x00 || info`) without allocating a copy.
    expand_one_key(
        ikm,
        &[
            LABEL_PREFIX.as_bytes(),
            label.purpose().as_bytes(),
            &[0u8],
            info,
        ],
        &mut okm,
    );
    Secret32::from_bytes(*okm)
}

/// HKDF-Extract with [`HKDF_SALT`], then HKDF-Expand into a 32-byte buffer.
// HKDF-SHA256 can expand up to 255 * 32 bytes, so a 32-byte output cannot fail;
// the assertion turns a future change of the length into a build error rather than
// a runtime failure.
#[allow(clippy::expect_used)]
fn expand_one_key(ikm: &Secret32, info: &[&[u8]], okm: &mut [u8; DERIVED_KEY_LEN]) {
    const _: () = assert!(DERIVED_KEY_LEN <= 255 * 32);
    hkdf_sha256(HKDF_SALT, ikm.expose(), info, okm).expect("HKDF-SHA256 supports a 32-byte output");
}

/// Textbook HKDF-SHA256 (RFC 5869): extract with `salt`, then expand `info` (the
/// concatenation of its parts) into `okm`. [`derive_key`] is this function with our
/// salt and label framing; the RFC known-answer test exercises it directly.
///
/// Fails only when `okm` is longer than 255 * 32 bytes.
pub(crate) fn hkdf_sha256(
    salt: &[u8],
    ikm: &[u8],
    info: &[&[u8]],
    okm: &mut [u8],
) -> Result<(), CryptoError> {
    let len = okm.len();
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand_multi_info(info, okm)
        .map_err(|_| CryptoError::Encoding(format!("HKDF-SHA256 cannot expand to {len} bytes")))
}

/// Hash chain step: `BLAKE3(prev || data)`.
#[must_use]
pub fn chain(prev: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(prev);
    hasher.update(data);
    *hasher.finalize().as_bytes()
}

/// BLAKE3 hash of `data`.
#[must_use]
pub fn hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Incremental BLAKE3 hashing, for data that arrives in pieces (attachment uploads and
/// reads). Same digest as [`hash`] over the concatenation.
#[derive(Debug, Clone, Default)]
pub struct Hasher(blake3::Hasher);

impl Hasher {
    /// A fresh hasher.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds more data.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// The digest of everything fed so far.
    #[must_use]
    pub fn finalize(&self) -> [u8; 32] {
        *self.0.finalize().as_bytes()
    }
}

/// The domain of the space key commitment (ADR-0044): `kb_types::sharing::SK_COMMIT_DOMAIN`.
pub const SK_COMMITMENT_DOMAIN: &str = kb_types::sharing::SK_COMMIT_DOMAIN;

/// The commitment to a shared space's key (ADR-0044): `BLAKE3(SK_COMMITMENT_DOMAIN || feed ||
/// epoch || sk)`, with `feed` as its 16 bytes and `epoch` as a little-endian `u32`. Every
/// recipient of a `Create`, `Rekey` or `Grant` box checks its opened key against the commitment in
/// the entry, so nobody can give different keys to different members.
///
/// The domain has a fixed length and so do the other three parts, so the input is unambiguous.
/// The result is public (it travels in clear in the membership log) and says nothing about the
/// key: BLAKE3 of a uniformly random 256-bit key.
#[must_use]
pub fn sk_commitment(feed: kb_types::FeedId, epoch: u32, sk: &Secret32) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SK_COMMITMENT_DOMAIN.as_bytes());
    hasher.update(feed.as_bytes());
    hasher.update(&epoch.to_le_bytes());
    hasher.update(sk.expose());
    *hasher.finalize().as_bytes()
}

/// Key check value: first 8 bytes of `BLAKE3("kcv" || key)`. Distinguishes "wrong
/// key" from "corrupted data" in diagnostics without weakening the key.
#[must_use]
pub fn key_check_value(key: &Secret32) -> [u8; 8] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(KCV_DOMAIN);
    hasher.update(key.expose());
    let digest = hasher.finalize();
    let mut kcv = [0u8; 8];
    kcv.copy_from_slice(&digest.as_bytes()[..8]);
    kcv
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use hkdf::hmac::{Hmac, KeyInit, Mac};
    use kb_types::testing::SeededRng;

    /// The frozen known-answer file; regenerated only by
    /// `cargo run -p kb-crypto --example make_vectors`.
    const VECTORS_JSON: &str = include_str!("../../../fixtures/crypto/derive-key-v1.json");

    /// RFC 5869 Appendix A.1-A.3, transcribed from the RFC.
    const HKDF_KAT_JSON: &str = include_str!("../../../fixtures/crypto/kat-hkdf-sha256.json");

    /// Every label, in the order of `docs/security/encryption-design.md`. Keep in sync
    /// with the constants on [`Label`].
    const ALL_LABELS: [Label; 21] = [
        Label::SPACE_KEK,
        Label::ACCOUNT_DOC,
        Label::LOCAL_DB,
        Label::KEYRING,
        Label::KEYRING_AUTH,
        Label::RECOVERY_WRAP,
        Label::RECOVERY_ID,
        Label::RECOVERY_SIGN,
        Label::PAIRING_SAS,
        Label::PAIRING_CHANNEL,
        Label::BACKUP,
        Label::DEVICE_NAME,
        Label::RECOVERY_KIT,
        Label::MEMBERSHIP_META,
        Label::SPACE_KEYRING,
        Label::INVITE_ID,
        Label::INVITE_SIGN,
        Label::INVITE_BUNDLE,
        Label::PRESENCE,
        Label::LINK_ID,
        Label::PUBLICATION,
    ];

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        assert_eq!(text.len() % 2, 0, "odd number of hex digits");
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn secret_from(bytes: &[u8]) -> Secret32 {
        Secret32::from_bytes(bytes.try_into().unwrap())
    }

    /// The key `00 01 02 .. 1f`.
    fn ramp() -> Secret32 {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap();
        }
        Secret32::from_bytes(bytes)
    }

    #[test]
    fn framing_constants_are_frozen() {
        assert_eq!(LABEL_PREFIX, "atomik-kb/v1/");
        assert_eq!(HKDF_SALT, b"atomik-kb/v1");
    }

    #[test]
    fn label_set_is_the_documented_one() {
        let mut purposes: Vec<&str> = ALL_LABELS.iter().map(|l| l.purpose()).collect();
        purposes.sort_unstable();
        let mut documented = vec![
            "space-kek",
            "account-doc",
            "local-db",
            "keyring",
            "keyring-auth",
            "recovery-wrap",
            "recovery-id",
            "recovery-sign",
            "pairing-sas",
            "pairing-channel",
            "backup",
            "device-name",
            "recovery-kit",
            "membership-meta",
            "space-keyring",
            "invite-id",
            "invite-sign",
            "invite-bundle",
            "presence",
            "link-id",
            "publication",
        ];
        documented.sort_unstable();
        assert_eq!(purposes, documented);
        for label in ALL_LABELS {
            assert!(
                label
                    .purpose()
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'-'),
                "purposes are lowercase ASCII words; the 0x00 separator relies on it"
            );
        }
    }

    #[test]
    fn derive_key_matches_the_frozen_vector_file() {
        let file: serde_json::Value = serde_json::from_str(VECTORS_JSON).unwrap();
        assert_eq!(file["suite"], 1);
        assert_eq!(file["label_prefix"], LABEL_PREFIX);
        assert_eq!(file["salt_hex"], hex(HKDF_SALT));

        let vectors = file["vectors"].as_array().unwrap();
        assert!(vectors.len() >= 4, "the file freezes at least four vectors");
        for vector in vectors {
            let name = vector["name"].as_str().unwrap();
            let ikm = secret_from(&unhex(vector["ikm_hex"].as_str().unwrap()));
            let purpose = vector["label"].as_str().unwrap();
            let label = ALL_LABELS
                .into_iter()
                .find(|l| l.purpose() == purpose)
                .unwrap();
            let info = unhex(vector["info_hex"].as_str().unwrap());
            let expected = vector["okm_hex"].as_str().unwrap();

            let okm = derive_key(&ikm, label, &info);
            assert_eq!(hex(okm.expose()), expected, "vector {name:?}");
        }
    }

    #[test]
    fn derive_key_is_textbook_hkdf_with_our_framing() {
        // Re-derive with HMAC directly (RFC 5869: extract, then the single expand
        // block that a 32-byte output needs) so the framing is verified independently
        // of the `hkdf` crate's expand code.
        let ikm = ramp();
        let long_info = [0u8; 100];
        let infos: [&[u8]; 3] = [b"", b"x", &long_info];
        for label in ALL_LABELS {
            for info in infos {
                let mut extract = Hmac::<Sha256>::new_from_slice(HKDF_SALT).unwrap();
                extract.update(ikm.expose());
                let prk = extract.finalize().into_bytes();

                let mut expand = Hmac::<Sha256>::new_from_slice(prk.as_slice()).unwrap();
                expand.update(LABEL_PREFIX.as_bytes());
                expand.update(label.purpose().as_bytes());
                expand.update(&[0u8]);
                expand.update(info);
                expand.update(&[1u8]);
                let expected = expand.finalize().into_bytes();

                assert_eq!(
                    derive_key(&ikm, label, info).expose().as_slice(),
                    expected.as_slice(),
                    "label {}",
                    label.purpose()
                );
            }
        }
    }

    #[test]
    fn hkdf_sha256_matches_rfc5869_appendix_a() {
        let file: serde_json::Value = serde_json::from_str(HKDF_KAT_JSON).unwrap();
        let vectors = file["vectors"].as_array().unwrap();
        assert_eq!(vectors.len(), 3, "A.1, A.2 and A.3");
        for vector in vectors {
            let name = vector["name"].as_str().unwrap();
            let ikm = unhex(vector["ikm_hex"].as_str().unwrap());
            let salt = unhex(vector["salt_hex"].as_str().unwrap());
            let info = unhex(vector["info_hex"].as_str().unwrap());
            let length = usize::try_from(vector["length"].as_u64().unwrap()).unwrap();
            let prk = vector["prk_hex"].as_str().unwrap();
            let okm = vector["okm_hex"].as_str().unwrap();

            let mut out = vec![0u8; length];
            hkdf_sha256(&salt, &ikm, &[&info], &mut out).unwrap();
            assert_eq!(hex(&out), okm, "{name}: OKM");

            // Info given in parts is the concatenation of the parts.
            let (first, second) = info.split_at(info.len() / 2);
            let mut parts = vec![0u8; length];
            hkdf_sha256(&salt, &ikm, &[first, &[], second], &mut parts).unwrap();
            assert_eq!(parts, out, "{name}: info parts");

            let (extracted, _) = Hkdf::<Sha256>::extract(Some(&salt), &ikm);
            assert_eq!(hex(&extracted), prk, "{name}: PRK");
        }
    }

    #[test]
    fn hkdf_sha256_refuses_outputs_longer_than_255_blocks() {
        let mut longest = vec![0u8; 255 * 32];
        assert!(hkdf_sha256(b"salt", b"ikm", &[b"info"], &mut longest).is_ok());
        let mut too_long = vec![0u8; 255 * 32 + 1];
        assert!(matches!(
            hkdf_sha256(b"salt", b"ikm", &[b"info"], &mut too_long),
            Err(CryptoError::Encoding(_))
        ));
    }

    #[test]
    fn derive_key_is_deterministic() {
        let ikm = ramp();
        let a = derive_key(&ikm, Label::LOCAL_DB, b"install");
        let b = derive_key(&ikm, Label::LOCAL_DB, b"install");
        assert_eq!(a, b);
        assert_ne!(a.expose(), ikm.expose(), "output is not the input key");
        assert_ne!(a.expose(), &[0u8; 32], "output is not all zero");
    }

    #[test]
    fn different_labels_give_different_keys() {
        let ikm = ramp();
        let keys: Vec<Secret32> = ALL_LABELS
            .iter()
            .map(|label| derive_key(&ikm, *label, b"same info"))
            .collect();
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "two labels collided");
            }
        }
    }

    #[test]
    fn different_infos_and_different_keys_give_different_keys() {
        let ikm = ramp();
        let infos: [&[u8]; 6] = [b"", b"\x00", b"\x01", b"a", b"ab", b"ba"];
        let keys: Vec<Secret32> = infos
            .iter()
            .map(|info| derive_key(&ikm, Label::LOCAL_DB, info))
            .collect();
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "two infos collided");
            }
        }

        let mut other = *ikm.expose();
        other[31] ^= 1;
        assert_ne!(
            derive_key(&ikm, Label::LOCAL_DB, b"x"),
            derive_key(&Secret32::from_bytes(other), Label::LOCAL_DB, b"x"),
            "a one-bit change of the input key changes the output"
        );
    }

    #[test]
    fn label_and_info_boundary_is_unambiguous() {
        let ikm = ramp();
        assert_ne!(
            derive_key(&ikm, Label::KEYRING, b"-auth"),
            derive_key(&ikm, Label::KEYRING_AUTH, b""),
            "`keyring` + `-auth` must not equal `keyring-auth`"
        );
    }

    #[test]
    fn key_check_value_vector() {
        // Frozen: first 8 bytes of BLAKE3("kcv" || 00 01 02 .. 1f).
        assert_eq!(hex(&key_check_value(&ramp())), "6b51a7c2d4dfd217");
        assert_eq!(
            hex(&key_check_value(&Secret32::from_bytes([0xff; 32]))),
            "6e17ae4b3fbf88b1"
        );
        // The definition, spelled out with the one-shot hash.
        let mut input = b"kcv".to_vec();
        input.extend_from_slice(ramp().expose());
        assert_eq!(key_check_value(&ramp()), hash(&input)[..8]);
    }

    #[test]
    fn key_check_value_distinguishes_keys_and_hides_the_key() {
        let a = ramp();
        let mut flipped = *a.expose();
        flipped[0] ^= 1;
        let b = Secret32::from_bytes(flipped);
        assert_ne!(key_check_value(&a), key_check_value(&b));
        assert_ne!(
            key_check_value(&a).as_slice(),
            &a.expose()[..8],
            "the check value is a hash, not a prefix of the key"
        );
    }

    #[test]
    fn the_shared_space_labels_derive_frozen_keys() {
        // M13 labels, ikm 00 01 .. 1f, info = 16 bytes 10 11 .. 1f (a feed id or empty).
        // `derive_key_is_textbook_hkdf_with_our_framing` re-derives every label with HMAC;
        // these frozen values make a changed label text or framing a test failure.
        let info: Vec<u8> = (0x10u8..0x20).collect();
        let cases: [(Label, &[u8], &str); 8] = [
            (
                Label::MEMBERSHIP_META,
                &info,
                "bbe1145a609cc7b03c324a8a98a462e6e224be0ae2c109bae4d3acfdc6a20b38",
            ),
            (
                Label::SPACE_KEYRING,
                &info,
                "d8b04e8d10a3b321ea1fe8bcb5b78cc1f38a7cf31075539f9744c3025dfd3ad6",
            ),
            (
                Label::INVITE_ID,
                b"",
                "27877c8ae786b7bfff6aa3ffebee604a3fe6cddb711f6fbac4f74777985d4791",
            ),
            (
                Label::INVITE_SIGN,
                b"",
                "6eeb000c01018a44bd0dd13cd406b7982d803336a797c38d1b33c6a9a56a61b3",
            ),
            (
                Label::INVITE_BUNDLE,
                b"",
                "55d5e4214cc84415537d416a60417de992df27457b02cd4411a7dbab02eddb6d",
            ),
            (
                // Slice 3 (ADR-0050): the key of presence frames, ikm = a space key, info = the feed id.
                Label::PRESENCE,
                &info,
                "18f688044d8f4b959b3180d73917c024b61b5954bde966f35d60975ab7bbbee3",
            ),
            (
                // Slice 4 (ADR-0051): the id of a view link, ikm = the link key, no info; the id is the
                // first 16 bytes.
                Label::LINK_ID,
                b"",
                "1483df439bde69b52d0148d5a52d933b88c6e5759edd5b434307a13325461fc4",
            ),
            (
                // Slice 4: the key of a view link's publication, ikm = the link key, no info.
                Label::PUBLICATION,
                b"",
                "2da85e96eb217d756e459f45f60bd32522124cba6220a392e6ea0ff0c996697e",
            ),
        ];
        for (label, info, expected) in cases {
            assert_eq!(
                hex(derive_key(&ramp(), label, info).expose()),
                expected,
                "label {}",
                label.purpose()
            );
        }
    }

    #[test]
    fn the_presence_label_is_the_shared_constant() {
        assert_eq!(Label::PRESENCE.purpose(), kb_types::sharing::PRESENCE_LABEL);
    }

    #[test]
    fn the_view_link_labels_are_the_shared_constants() {
        assert_eq!(Label::LINK_ID.purpose(), kb_types::sharing::LINK_ID_LABEL);
        assert_eq!(
            Label::PUBLICATION.purpose(),
            kb_types::sharing::PUBLICATION_LABEL
        );
        // The two keys of one link key are unrelated.
        assert_ne!(
            derive_key(&ramp(), Label::LINK_ID, b""),
            derive_key(&ramp(), Label::PUBLICATION, b"")
        );
    }

    #[test]
    fn sk_commitment_is_blake3_of_domain_feed_epoch_and_key() {
        assert_eq!(SK_COMMITMENT_DOMAIN, "atomik-kb/v1/sk-commit");
        let mut feed_bytes = [0u8; 16];
        for (i, b) in feed_bytes.iter_mut().enumerate() {
            *b = 0x10 + u8::try_from(i).unwrap();
        }
        let feed = kb_types::FeedId::from_bytes(feed_bytes);

        // The formula spelled out by hand: domain, the feed's 16 bytes, the epoch as a
        // little-endian u32 (258 = 0x0102 is 02 01 00 00), the 32 key bytes.
        let mut input = b"atomik-kb/v1/sk-commit".to_vec();
        input.extend_from_slice(&feed_bytes);
        input.extend_from_slice(&[0x02, 0x01, 0x00, 0x00]);
        input.extend_from_slice(ramp().expose());
        assert_eq!(input.len(), 22 + 16 + 4 + 32);
        assert_eq!(sk_commitment(feed, 258, &ramp()), hash(&input));

        // Frozen (cross-checked above against the hand-built input).
        assert_eq!(
            hex(&sk_commitment(feed, 258, &ramp())),
            "834df1723bad17662814de3b1ce699da0d10f92d845e94782496e8ecbd5dd60a"
        );
    }

    #[test]
    fn sk_commitment_binds_the_feed_the_epoch_and_the_key() {
        let feed = kb_types::FeedId::from_bytes([7; 16]);
        let base = sk_commitment(feed, 1, &ramp());
        assert_eq!(base, sk_commitment(feed, 1, &ramp()), "deterministic");
        assert_ne!(base, sk_commitment(feed, 2, &ramp()), "the epoch");
        assert_ne!(
            sk_commitment(feed, 1, &ramp()),
            sk_commitment(feed, 1 << 24, &ramp()),
            "the epoch is little-endian, not truncated"
        );
        let other_feed = kb_types::FeedId::from_bytes([8; 16]);
        assert_ne!(base, sk_commitment(other_feed, 1, &ramp()), "the feed");
        let mut flipped = *ramp().expose();
        flipped[31] ^= 1;
        assert_ne!(
            base,
            sk_commitment(feed, 1, &Secret32::from_bytes(flipped)),
            "the key"
        );
        assert_ne!(
            base.as_slice(),
            ramp().expose().as_slice(),
            "a hash, not the key"
        );
    }

    #[test]
    fn hash_is_blake3() {
        // Official BLAKE3 test vectors for inputs of 0 and 1 bytes.
        assert_eq!(
            hex(&hash(b"")),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(
            hex(&hash(&[0u8])),
            "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"
        );
    }

    #[test]
    fn secret32_debug_never_prints_bytes() {
        let secret = Secret32::from_bytes([0xAB; 32]);
        for text in [format!("{secret:?}"), format!("{secret:#?}")] {
            assert_eq!(text.trim(), "Secret32(<redacted>)");
            for needle in ["ab", "AB", "171", "0xab", "0xAB"] {
                assert!(!text.contains(needle), "{needle} leaked in {text}");
            }
        }
        // A secret nested in a derived Debug stays redacted too.
        #[derive(Debug)]
        #[allow(dead_code)] // only read through the derived Debug impl
        struct Holder(Secret32);
        assert_eq!(
            format!("{:?}", Holder(secret)),
            "Holder(Secret32(<redacted>))"
        );
    }

    #[test]
    fn secret32_zeroize_clears_the_bytes() {
        let mut secret = Secret32::from_bytes([0x5A; 32]);
        secret.zeroize();
        assert_eq!(secret.expose(), &[0u8; 32]);
    }

    #[test]
    fn secret32_generate_draws_32_bytes_from_the_rng() {
        let a = Secret32::generate(&SeededRng::new(1));
        let b = Secret32::generate(&SeededRng::new(1));
        let c = Secret32::generate(&SeededRng::new(2));
        assert_eq!(a, b, "same seed, same key");
        assert_ne!(a, c, "different seed, different key");
        assert_ne!(a.expose(), &[0u8; 32]);
    }
}
