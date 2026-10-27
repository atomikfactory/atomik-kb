//! The recovery key (RcK) and what derives from it (ADR-0006, ADR-0017,
//! `key-management.md`): the kit encodings, the recovery id, the recovery signing
//! key and the wrapped root key.
//!
//! Derivations (HKDF-SHA256, empty info): `recovery_id = HKDF(RcK, recovery-id)`,
//! recovery signing seed `= HKDF(RcK, recovery-sign)`, root-key wrapping key
//! `= HKDF(RcK, recovery-wrap)`; the wrapped ARK names the recovery id as its subject.

use kb_crypto::{CryptoError, Label, Secret32, SigningKey, VerifyingKey, WrappedKey, kit};
use kb_types::AccountId;
use kb_types::Rng;

use crate::{Ark, KeysError};

/// The recovery key: 32 random bytes that only ever exist on the printed kit (and in
/// memory while a kit is shown or typed).
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryKey(Secret32);

impl RecoveryKey {
    /// Fresh random key.
    #[must_use]
    pub fn generate(rng: &dyn Rng) -> Self {
        Self(Secret32::generate(rng))
    }

    /// Wraps raw bytes (tests, fixtures).
    #[must_use]
    pub const fn from_secret(secret: Secret32) -> Self {
        Self(secret)
    }

    /// Parses a typed kit (words or code, surrounding whitespace ignored);
    /// [`KeysError::InvalidKit`] otherwise. The error says what is wrong, never what
    /// was typed.
    pub fn from_kit(text: &str) -> Result<Self, KeysError> {
        kit::parse(text.trim())
            .map(Self)
            .map_err(|error| match error {
                CryptoError::InvalidKit(reason) => KeysError::InvalidKit(reason),
                CryptoError::Authentication | CryptoError::WeakParameters => {
                    KeysError::InvalidKit("the kit does not decode".to_owned())
                }
                CryptoError::Encoding(reason) => KeysError::InvalidKit(reason),
            })
    }

    /// The 24 words.
    #[must_use]
    pub fn words(&self) -> Vec<String> {
        kit::words(&self.0)
            .iter()
            .map(|word| (*word).to_owned())
            .collect()
    }

    /// The code form.
    #[must_use]
    pub fn code(&self) -> String {
        kit::code(&self.0)
    }

    /// `HKDF(RcK, recovery-id)`: a non-secret identifier that lets a device verify a
    /// re-typed kit and lets a server find the backup blob.
    #[must_use]
    pub fn recovery_id(&self) -> [u8; 32] {
        *kb_crypto::derive_key(&self.0, Label::RECOVERY_ID, b"").expose()
    }

    /// The recovery signing key, seed `HKDF(RcK, recovery-sign)`.
    #[must_use]
    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_seed(&kb_crypto::derive_key(&self.0, Label::RECOVERY_SIGN, b""))
    }

    /// The public half of [`Self::signing_key`], as the device log records it.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key().verifying_key()
    }

    /// `wrap(ARK, HKDF(RcK, recovery-wrap))` with the recovery id as subject.
    #[must_use]
    pub fn wrap_ark(&self, ark: &Ark, rng: &dyn Rng) -> WrappedKey {
        kb_crypto::wrap_key(ark.secret(), &self.wrap_key(), &self.recovery_id(), rng)
    }

    /// Reverses [`Self::wrap_ark`]; a wrong kit is [`KeysError::Crypto`].
    pub fn unwrap_ark(&self, wrapped: &WrappedKey) -> Result<Ark, KeysError> {
        let secret = kb_crypto::unwrap_key(wrapped, &self.wrap_key(), &self.recovery_id())?;
        Ok(Ark::from_secret(secret))
    }

    /// The raw secret (for the kit encodings and tests).
    #[must_use]
    pub fn secret(&self) -> &Secret32 {
        &self.0
    }

    /// `HKDF(RcK, recovery-wrap)`.
    fn wrap_key(&self) -> Secret32 {
        kb_crypto::derive_key(&self.0, Label::RECOVERY_WRAP, b"")
    }
}

/// What a server stores under `recovery_id` (ADR-0022): the account and the root key
/// wrapped under the kit. Opaque to the server; only the kit opens the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryBlob {
    /// The account the blob belongs to.
    pub account_id: AccountId,
    /// Root key generation the wrapped key has.
    pub generation: u32,
    /// `RecoveryKey::wrap_ark` output.
    pub wrapped: WrappedKey,
}

/// Format version of [`RecoveryBlob::encode`].
pub const RECOVERY_BLOB_VERSION: u8 = 1;

impl RecoveryBlob {
    /// `version u8 || account_id [16] || generation u32 LE || wrapped key bytes`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let wrapped = self.wrapped.to_bytes();
        let mut out = Vec::with_capacity(1 + 16 + 4 + wrapped.len());
        out.push(RECOVERY_BLOB_VERSION);
        out.extend_from_slice(self.account_id.as_bytes());
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&wrapped);
        out
    }

    /// Parses [`Self::encode`]; anything else is [`KeysError::Corrupted`].
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let malformed = || KeysError::Corrupted("malformed recovery blob".to_owned());
        let (&version, rest) = bytes.split_first().ok_or_else(malformed)?;
        if version != RECOVERY_BLOB_VERSION {
            return Err(KeysError::Corrupted(format!(
                "unsupported recovery blob version {version}"
            )));
        }
        if rest.len() < 20 {
            return Err(malformed());
        }
        let (id, rest) = rest.split_at(16);
        let (generation, wrapped) = rest.split_at(4);
        let mut id_bytes = [0u8; 16];
        id_bytes.copy_from_slice(id);
        let mut gen_bytes = [0u8; 4];
        gen_bytes.copy_from_slice(generation);
        Ok(Self {
            account_id: AccountId::from_bytes(id_bytes),
            generation: u32::from_le_bytes(gen_bytes),
            wrapped: WrappedKey::from_bytes(wrapped).map_err(|_| malformed())?,
        })
    }
}

impl core::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RecoveryKey(<redacted>)")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::ark;
    use kb_types::testing::SeededRng;

    fn rck(seed: u64) -> RecoveryKey {
        RecoveryKey::generate(&SeededRng::new(seed))
    }

    #[test]
    fn derivations_use_their_own_labels() {
        let key = rck(1);
        let secret = key.secret();
        assert_eq!(
            key.recovery_id(),
            *kb_crypto::derive_key(secret, Label::RECOVERY_ID, b"").expose()
        );
        assert_eq!(
            key.signing_key().verifying_key(),
            SigningKey::from_seed(&kb_crypto::derive_key(secret, Label::RECOVERY_SIGN, b""))
                .verifying_key()
        );
        assert_eq!(key.verifying_key(), key.signing_key().verifying_key());

        // Three purposes, three unrelated values, none of them the key itself.
        let id = key.recovery_id();
        let seed = key.signing_key().to_seed();
        let kek = key.wrap_key();
        assert_ne!(&id, seed.expose());
        assert_ne!(&id, kek.expose());
        assert_ne!(seed.expose(), kek.expose());
        for derived in [&id, seed.expose(), kek.expose()] {
            assert_ne!(derived, secret.expose());
        }
    }

    #[test]
    fn derivations_are_deterministic_and_per_key() {
        let (a, b) = (rck(1), rck(2));
        assert_eq!(a.recovery_id(), rck(1).recovery_id());
        assert_eq!(a.verifying_key(), rck(1).verifying_key());
        assert_ne!(a.recovery_id(), b.recovery_id());
        assert_ne!(a.verifying_key(), b.verifying_key());
    }

    #[test]
    fn the_ark_round_trips_through_the_kit() {
        let key = rck(1);
        let root = ark(5);
        let rng = SeededRng::new(9);
        let wrapped = key.wrap_ark(&root, &rng);
        assert_eq!(key.unwrap_ark(&wrapped).unwrap(), root);

        // The documented construction: kek HKDF(RcK, recovery-wrap), subject = recovery id.
        let kek = kb_crypto::derive_key(key.secret(), Label::RECOVERY_WRAP, b"");
        assert_eq!(
            &kb_crypto::unwrap_key(&wrapped, &kek, &key.recovery_id()).unwrap(),
            root.secret()
        );
        assert!(kb_crypto::unwrap_key(&wrapped, &kek, b"").is_err());

        // Fresh nonce each time.
        assert_ne!(key.wrap_ark(&root, &rng), wrapped);
    }

    #[test]
    fn a_wrong_kit_does_not_unwrap_the_ark() {
        let wrapped = rck(1).wrap_ark(&ark(5), &SeededRng::new(9));
        let err = rck(2).unwrap_ark(&wrapped).unwrap_err();
        assert!(
            matches!(err, KeysError::Crypto(CryptoError::Authentication)),
            "{err:?}"
        );
        let mut tampered = wrapped.to_bytes();
        tampered[30] ^= 1;
        let tampered = WrappedKey::from_bytes(&tampered).unwrap();
        assert!(matches!(
            rck(1).unwrap_ark(&tampered),
            Err(KeysError::Crypto(CryptoError::Authentication))
        ));
    }

    #[test]
    fn words_and_code_decode_to_the_same_key() {
        let key = rck(1);
        let words = key.words();
        assert_eq!(words.len(), 24);
        assert_eq!(
            words,
            kit::words(key.secret())
                .iter()
                .map(|w| (*w).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(key.code(), kit::code(key.secret()));

        let from_words = RecoveryKey::from_kit(&words.join(" ")).unwrap();
        let from_code = RecoveryKey::from_kit(&key.code()).unwrap();
        assert_eq!(from_words, key);
        assert_eq!(from_code, key);
        assert_eq!(from_words.recovery_id(), from_code.recovery_id());

        // Surrounding whitespace, line breaks between words, upper case.
        let typed = format!("  {}\n", words.join("\n").to_uppercase());
        assert_eq!(RecoveryKey::from_kit(&typed).unwrap(), key);
        assert_eq!(
            RecoveryKey::from_kit(&format!("\t{} ", key.code().to_lowercase())).unwrap(),
            key
        );
    }

    #[test]
    fn a_mistyped_kit_is_invalid_and_does_not_echo_the_input() {
        let key = rck(1);
        let words = key.words();

        // Two different words swapped: every word valid, the checksum wrong.
        let j = (1..words.len()).find(|&j| words[j] != words[0]).unwrap();
        let mut swapped = words.clone();
        swapped.swap(0, j);
        // A token that is no word of the list but contains a real kit word.
        let typo_token = format!("{}zq", words[3]);
        let mut typo = words.clone();
        typo[3].clone_from(&typo_token);
        // The code with its first character changed.
        let code = key.code();
        let mut chars: Vec<char> = code.chars().collect();
        chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
        let bad_code: String = chars.into_iter().collect();

        let swapped_text = swapped.join(" ");
        let typo_text = typo.join(" ");
        let short_text = words[..23].join(" ");
        for text in [
            "",
            "   ",
            "not a kit at all",
            swapped_text.as_str(),
            typo_text.as_str(),
            short_text.as_str(),
            bad_code.as_str(),
        ] {
            let err = RecoveryKey::from_kit(text).unwrap_err();
            assert!(matches!(err, KeysError::InvalidKit(_)), "{text:?}: {err:?}");
            let message = err.to_string();
            assert!(
                !message.contains(&typo_token),
                "typed token echoed: {message}"
            );
            assert!(!message.contains(&code[..9]), "code echoed: {message}");
            assert!(!message.contains(&bad_code[1..9]), "code echoed: {message}");
            assert!(
                !message.contains(&format!("{} {}", words[1], words[2])),
                "words echoed: {message}"
            );
        }
    }

    fn blob() -> RecoveryBlob {
        RecoveryBlob {
            account_id: AccountId::from_bytes([0xac; 16]),
            generation: 0x0102_0304,
            wrapped: rck(1).wrap_ark(&ark(5), &SeededRng::new(9)),
        }
    }

    #[test]
    fn a_recovery_blob_round_trips_and_opens_with_the_kit() {
        let blob = blob();
        let bytes = blob.encode();
        assert_eq!(RecoveryBlob::decode(&bytes).unwrap(), blob);
        assert!(bytes.len() <= kb_types::devices::RECOVERY_BLOB_MAX_BYTES);
        let decoded = RecoveryBlob::decode(&bytes).unwrap();
        assert_eq!(rck(1).unwrap_ark(&decoded.wrapped).unwrap(), ark(5));
        assert!(rck(2).unwrap_ark(&decoded.wrapped).is_err());
    }

    #[test]
    fn a_recovery_blob_has_the_documented_layout() {
        let blob = blob();
        let mut expected = vec![RECOVERY_BLOB_VERSION];
        expected.extend_from_slice(&[0xac; 16]);
        expected.extend_from_slice(&[4, 3, 2, 1]);
        expected.extend_from_slice(&blob.wrapped.to_bytes());
        assert_eq!(blob.encode(), expected);
        assert_eq!(expected.len(), 1 + 16 + 4 + kb_crypto::WRAPPED_KEY_LEN);
        assert_eq!(RECOVERY_BLOB_VERSION, 1);
    }

    #[test]
    fn malformed_recovery_blobs_are_corrupted() {
        let bytes = blob().encode();
        for len in 0..bytes.len() {
            let err = RecoveryBlob::decode(&bytes[..len]).unwrap_err();
            assert!(
                matches!(err, KeysError::Corrupted(_)),
                "{len} bytes: {err:?}"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(
            RecoveryBlob::decode(&longer),
            Err(KeysError::Corrupted(_))
        ));
        for version in [0u8, 2, 0xff] {
            let mut other = bytes.clone();
            other[0] = version;
            let err = RecoveryBlob::decode(&other).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("corrupted key material: unsupported recovery blob version {version}")
            );
        }
        // Errors carry no input bytes.
        let message = RecoveryBlob::decode(&bytes[..30]).unwrap_err().to_string();
        assert_eq!(message, "corrupted key material: malformed recovery blob");
    }

    #[test]
    fn debug_output_is_redacted() {
        let key = RecoveryKey::from_secret(Secret32::from_bytes([0xAB; 32]));
        assert_eq!(format!("{key:?}"), "RecoveryKey(<redacted>)");
    }
}
