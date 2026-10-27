//! Device keys: an Ed25519 signing key and an X25519 agreement key per installation,
//! kept as 32-byte seeds in the keystore (`key-management.md`, "Key hierarchy").
//!
//! The seeds live in two entries, `dsk:<install id>` (signing) and `dak:<install id>`
//! (agreement), written together at first open and read back before use, like the
//! ARK. The public halves are what the device log publishes.

use kb_crypto::{AgreementPublic, AgreementSecret, Secret32, SigningKey, VerifyingKey};
use kb_types::{InstallId, Rng};

use crate::{KeysError, Keystore, secret_from_stored, store_verified};

/// This device's private keys. Never printed.
#[derive(Clone)]
pub struct DeviceKeys {
    signing: SigningKey,
    agreement: AgreementSecret,
}

impl DeviceKeys {
    /// Keys from their stored seeds.
    #[must_use]
    pub fn from_seeds(signing_seed: &Secret32, agreement_seed: &Secret32) -> Self {
        Self {
            signing: SigningKey::from_seed(signing_seed),
            agreement: AgreementSecret::from_seed(agreement_seed),
        }
    }

    /// Fresh random keys: the signing seed is drawn first, then the agreement seed.
    #[must_use]
    pub fn generate(rng: &dyn Rng) -> Self {
        Self::from_seeds(&Secret32::generate(rng), &Secret32::generate(rng))
    }

    /// The signing key.
    #[must_use]
    pub fn signing(&self) -> &SigningKey {
        &self.signing
    }

    /// The key-agreement key.
    #[must_use]
    pub fn agreement(&self) -> &AgreementSecret {
        &self.agreement
    }

    /// The public halves, as they appear in the device log.
    #[must_use]
    pub fn public(&self) -> DevicePublicKeys {
        DevicePublicKeys {
            signing: self.signing.verifying_key(),
            agreement: self.agreement.public(),
        }
    }
}

impl core::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("DeviceKeys(<redacted>)")
    }
}

/// A device's public keys.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DevicePublicKeys {
    /// Ed25519 verifying key.
    pub signing: VerifyingKey,
    /// X25519 public key.
    pub agreement: AgreementPublic,
}

/// Keystore entry holding the signing seed.
#[must_use]
pub fn signing_entry_name(install_id: &InstallId) -> String {
    format!("dsk:{install_id}")
}

/// Keystore entry holding the agreement seed.
#[must_use]
pub fn agreement_entry_name(install_id: &InstallId) -> String {
    format!("dak:{install_id}")
}

/// Loads both seeds from the keystore or creates and stores fresh ones (read back
/// before use, like the ARK). A present but malformed entry is
/// [`KeysError::Corrupted`] and left untouched.
///
/// The two entries exist together or not at all: when only one is present the keys
/// cannot be restored, which is [`KeysError::Corrupted`] too (the present entry is
/// left untouched; replacing it would change this device's identity behind the
/// device log's back). Creation draws the signing seed, then the agreement seed, from
/// `rng`; if either write fails or does not read back intact, both new entries are
/// removed again (best effort) so that a retry starts from a clean state. Seed bytes
/// never appear in logs or errors.
pub fn load_or_create_device_keys(
    keystore: &dyn Keystore,
    rng: &dyn Rng,
    install_id: &InstallId,
) -> Result<DeviceKeys, KeysError> {
    let signing_name = signing_entry_name(install_id);
    let agreement_name = agreement_entry_name(install_id);

    let signing = keystore.get(&signing_name)?;
    let agreement = keystore.get(&agreement_name)?;
    match (signing, agreement) {
        (Some(signing), Some(agreement)) => {
            let signing_seed = secret_from_stored(&signing, "device signing seed")?;
            let agreement_seed = secret_from_stored(&agreement, "device agreement seed")?;
            Ok(DeviceKeys::from_seeds(&signing_seed, &agreement_seed))
        }
        (Some(_), None) => Err(KeysError::Corrupted(
            "the device agreement seed is missing from the keystore".to_owned(),
        )),
        (None, Some(_)) => Err(KeysError::Corrupted(
            "the device signing seed is missing from the keystore".to_owned(),
        )),
        (None, None) => create_device_keys(keystore, rng, &signing_name, &agreement_name),
    }
}

/// Draws, stores and reads back both seeds; removes both entries again on failure.
fn create_device_keys(
    keystore: &dyn Keystore,
    rng: &dyn Rng,
    signing_name: &str,
    agreement_name: &str,
) -> Result<DeviceKeys, KeysError> {
    let signing_seed = Secret32::generate(rng);
    let agreement_seed = Secret32::generate(rng);

    let stored = store_verified(
        keystore,
        signing_name,
        signing_seed.expose(),
        "device signing seed",
    )
    .and_then(|()| {
        store_verified(
            keystore,
            agreement_name,
            agreement_seed.expose(),
            "device agreement seed",
        )
    });
    if let Err(error) = stored {
        // Neither entry existed before this call, so removing both cannot lose a key.
        let _ = keystore.delete(signing_name);
        let _ = keystore.delete(agreement_name);
        return Err(error);
    }

    tracing::info!("created new device keys in the keystore");
    Ok(DeviceKeys::from_seeds(&signing_seed, &agreement_seed))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::{Faulty, Forgetful, Unavailable, install_id, other_install_id};
    use crate::{KeystoreError, MemoryKeystore};
    use kb_types::testing::SeededRng;

    fn names() -> (String, String) {
        let id = install_id();
        (signing_entry_name(&id), agreement_entry_name(&id))
    }

    #[test]
    fn entry_names() {
        let id = install_id();
        assert_eq!(
            signing_entry_name(&id),
            "dsk:12345678-9abc-4def-8123-456789abcdef"
        );
        assert_eq!(
            agreement_entry_name(&id),
            "dak:12345678-9abc-4def-8123-456789abcdef"
        );
    }

    #[test]
    fn creates_stores_and_loads_the_same_keys() {
        let keystore = MemoryKeystore::new();
        let (dsk, dak) = names();

        let created =
            load_or_create_device_keys(&keystore, &SeededRng::new(1), &install_id()).unwrap();
        let signing_seed = keystore.get(&dsk).unwrap().unwrap();
        let agreement_seed = keystore.get(&dak).unwrap().unwrap();
        assert_eq!(signing_seed.len(), 32);
        assert_eq!(agreement_seed.len(), 32);
        assert_ne!(signing_seed.as_slice(), agreement_seed.as_slice());
        assert_eq!(
            signing_seed.as_slice(),
            created.signing().to_seed().expose(),
            "the signing entry holds the signing seed"
        );
        assert_eq!(
            agreement_seed.as_slice(),
            created.agreement().to_seed().expose(),
            "the agreement entry holds the agreement seed"
        );

        // A different seed proves the second call loaded instead of regenerating.
        let loaded =
            load_or_create_device_keys(&keystore, &SeededRng::new(99), &install_id()).unwrap();
        assert_eq!(loaded.public(), created.public());
    }

    #[test]
    fn new_keys_are_drawn_from_the_injected_rng() {
        let keystore = MemoryKeystore::new();
        let keys =
            load_or_create_device_keys(&keystore, &SeededRng::new(5), &install_id()).unwrap();
        assert_eq!(
            keys.public(),
            DeviceKeys::generate(&SeededRng::new(5)).public(),
            "signing seed first, agreement seed second"
        );
    }

    #[test]
    fn each_install_has_its_own_keys() {
        let keystore = MemoryKeystore::new();
        let rng = SeededRng::new(3);
        let a = load_or_create_device_keys(&keystore, &rng, &install_id()).unwrap();
        let b = load_or_create_device_keys(&keystore, &rng, &other_install_id()).unwrap();
        assert_ne!(a.public(), b.public());
        let a_again = load_or_create_device_keys(&keystore, &rng, &install_id()).unwrap();
        assert_eq!(a_again.public(), a.public());
    }

    #[test]
    fn malformed_entries_are_corrupted_and_left_untouched() {
        let (dsk, dak) = names();
        for (bad_name, good_name) in [(&dsk, &dak), (&dak, &dsk)] {
            for len in [0usize, 1, 31, 33, 64] {
                let keystore = MemoryKeystore::new();
                let junk = vec![0xAB; len];
                keystore.set(bad_name, &junk).unwrap();
                keystore.set(good_name, &[7; 32]).unwrap();

                let err = load_or_create_device_keys(&keystore, &SeededRng::new(1), &install_id())
                    .unwrap_err();
                let message = err.to_string();
                assert!(matches!(err, KeysError::Corrupted(_)), "{message}");
                assert!(message.contains(&len.to_string()), "length is reported");
                for leaked in ["abab", "0xab", "171", "0707"] {
                    assert!(!message.contains(leaked), "{leaked} leaked: {message}");
                }
                assert_eq!(keystore.get(bad_name).unwrap().unwrap().as_slice(), junk);
                assert_eq!(
                    keystore.get(good_name).unwrap().unwrap().as_slice(),
                    [7; 32]
                );
            }
        }
    }

    #[test]
    fn one_missing_entry_is_corrupted_and_the_other_is_left_untouched() {
        let (dsk, dak) = names();
        for present in [&dsk, &dak] {
            let keystore = MemoryKeystore::new();
            keystore.set(present, &[9; 32]).unwrap();

            let err = load_or_create_device_keys(&keystore, &SeededRng::new(1), &install_id())
                .unwrap_err();
            assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
            assert_eq!(keystore.get(present).unwrap().unwrap().as_slice(), [9; 32]);
            let absent = if present == &dsk { &dak } else { &dsk };
            assert!(
                keystore.get(absent).unwrap().is_none(),
                "nothing is recreated next to a lone entry"
            );
        }
    }

    #[test]
    fn a_mangling_keystore_is_detected_and_nothing_is_left_behind() {
        let (dsk, dak) = names();
        // Mangle every write, then only the second one.
        for mangle in ["", "dak:"] {
            let keystore = Faulty {
                mangle_set: Some(mangle),
                ..Faulty::default()
            };
            let err = load_or_create_device_keys(&keystore, &SeededRng::new(1), &install_id())
                .unwrap_err();
            assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
            assert!(keystore.inner.get(&dsk).unwrap().is_none(), "{mangle:?}");
            assert!(keystore.inner.get(&dak).unwrap().is_none(), "{mangle:?}");
        }
    }

    #[test]
    fn a_failed_second_write_removes_the_first_entry() {
        let (dsk, dak) = names();
        let keystore = Faulty {
            fail_set: Some("dak:"),
            ..Faulty::default()
        };
        let err =
            load_or_create_device_keys(&keystore, &SeededRng::new(1), &install_id()).unwrap_err();
        assert!(
            matches!(&err, KeysError::Keystore(KeystoreError::Other(m)) if m == "write refused"),
            "{err:?}"
        );
        assert!(keystore.inner.get(&dsk).unwrap().is_none());
        assert!(keystore.inner.get(&dak).unwrap().is_none());

        // The next attempt on a working keystore starts from scratch.
        let keys = load_or_create_device_keys(&keystore.inner, &SeededRng::new(2), &install_id());
        assert!(keys.is_ok());
    }

    #[test]
    fn a_forgetful_keystore_is_detected() {
        let err =
            load_or_create_device_keys(&Forgetful, &SeededRng::new(1), &install_id()).unwrap_err();
        assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
    }

    #[test]
    fn keystore_failures_propagate() {
        let err = load_or_create_device_keys(&Unavailable, &SeededRng::new(1), &install_id())
            .unwrap_err();
        assert!(
            matches!(
                &err,
                KeysError::Keystore(KeystoreError::Unavailable(m)) if m == "no secret service"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn debug_output_is_redacted() {
        let keys = DeviceKeys::from_seeds(
            &Secret32::from_bytes([0xAB; 32]),
            &Secret32::from_bytes([0xCD; 32]),
        );
        let text = format!("{keys:?} {keys:#?}");
        assert!(text.contains("DeviceKeys(<redacted>)"));
        for leaked in ["ab", "AB", "cd", "CD", "171", "205"] {
            assert!(!text.contains(leaked), "{leaked} leaked: {text}");
        }
    }
}
