//! Key hierarchy (`docs/security/key-management.md`).
//!
//! Milestone 1: the Account Root Key (ARK) in the platform keystore and the local
//! database key derived from it. Milestone 4: device keys ([`device`]), the device log
//! ([`log`]), the recovery key and kit ([`recovery`]), the keyring ([`keyring`]), the
//! app lock ([`lock`]) and root key rotation ([`rotation`]). Milestone 13 (ADR-0041 to
//! ADR-0046): the membership log and its fold ([`membership`]), the keys of a shared space
//! ([`space_keys`]), invites ([`invite`]) and safety numbers ([`safety`]); the pairing session
//! ([`pairing`]) serves invites too. Slice 4 (ADR-0051): view links, the key in the URL fragment
//! and what derives from it ([`link`]) and the `Link` subject of the keyring ([`keyring`]).
//!
//! Everything here is pure: platform access goes through [`Keystore`] and the
//! injected [`Rng`]; key material lives in [`Secret32`] (zeroised on drop, redacted in
//! `Debug`) and never appears in logs or error messages.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod codec;
pub mod device;
pub mod invite;
pub mod keyring;
pub mod link;
pub mod lock;
pub mod log;
pub mod membership;
pub mod pairing;
pub mod recovery;
pub mod rotation;
pub mod safety;
pub mod space_keys;
#[cfg(test)]
mod test_support;

pub use device::{DeviceKeys, DevicePublicKeys, load_or_create_device_keys};
pub use keyring::{KeyRecord, KeySubject, Keyring, KeyringCounts, KeyringItem, Tombstone};
pub use link::{LINK_KEY_FRAGMENT_LEN, LinkKey};
pub use lock::{ArkState, LockedArk, PASSPHRASE_MAX_BYTES, PASSPHRASE_MIN_BYTES, load_ark_state};
pub use log::{
    DeviceLog, DeviceRecord, LogEntry, LogEntryBody, LogError, LogSigner, LogState, Signer,
};
pub use recovery::RecoveryKey;

use std::collections::HashMap;
use std::sync::Mutex;

use kb_crypto::{Label, Secret32};
use kb_types::{InstallId, MaybeSend, MaybeSync, Rng};
use zeroize::Zeroizing;

/// Length in bytes of the ARK as stored in the keystore.
const ARK_LEN: usize = 32;

/// Errors from the platform keystore.
#[derive(Debug, thiserror::Error)]
pub enum KeystoreError {
    /// No usable keystore on this machine (e.g. Linux without a secret service).
    #[error("keystore unavailable: {0}")]
    Unavailable(String),
    /// The keystore exists but the operation failed.
    #[error("keystore error: {0}")]
    Other(String),
}

/// Errors from key management.
#[derive(Debug, thiserror::Error)]
pub enum KeysError {
    /// Keystore failure.
    #[error(transparent)]
    Keystore(#[from] KeystoreError),
    /// Stored material has the wrong shape (e.g. not 32 bytes).
    #[error("corrupted key material: {0}")]
    Corrupted(String),
    /// A cryptographic operation failed.
    #[error(transparent)]
    Crypto(#[from] kb_crypto::CryptoError),
    /// The device log does not verify.
    #[error(transparent)]
    Log(#[from] log::LogError),
    /// A typed recovery kit that does not decode or does not match.
    #[error("invalid recovery kit: {0}")]
    InvalidKit(String),
    /// The app-lock passphrase is wrong.
    #[error("wrong passphrase")]
    WrongPassphrase,
    /// A passphrase outside the accepted length.
    #[error("passphrase must be {PASSPHRASE_MIN_BYTES} to {PASSPHRASE_MAX_BYTES} bytes")]
    PassphraseLength,
    /// The ARK is wrapped under a passphrase; unlock first.
    #[error("the root key is locked")]
    Locked,
}

/// Platform secret storage: Windows Credential Manager, macOS Keychain, Linux secret
/// service, Android Keystore-wrapped preferences, WebCrypto-wrapped IndexedDB.
///
/// Values are small (tens of bytes). Names are stable strings such as `ark:<install id>`.
pub trait Keystore: MaybeSend + MaybeSync {
    /// Reads a secret; `Ok(None)` when absent.
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError>;
    /// Creates or replaces a secret.
    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError>;
    /// Deletes a secret; succeeds when absent.
    fn delete(&self, name: &str) -> Result<(), KeystoreError>;
}

/// The Account Root Key: random 256 bits, never password-derived (ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ark(Secret32);

impl Ark {
    /// Wraps raw key bytes (from the keystore).
    #[must_use]
    pub const fn from_secret(secret: Secret32) -> Self {
        Self(secret)
    }

    /// The underlying secret.
    #[must_use]
    pub fn secret(&self) -> &Secret32 {
        &self.0
    }
}

/// Raw key for the local SQLCipher/sqlite3mc database, derived from the ARK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbKey(Secret32);

impl DbKey {
    /// The underlying secret.
    #[must_use]
    pub fn secret(&self) -> &Secret32 {
        &self.0
    }
}

/// Keystore entry name holding the ARK for this installation.
#[must_use]
pub fn ark_entry_name(install_id: &InstallId) -> String {
    format!("ark:{install_id}")
}

/// Loads the ARK from the keystore or creates and stores a new one.
///
/// The Milestone 1 entry point, kept for callers that do not handle the app lock: a
/// missing ARK is created ([`lock::create_ark`]), a plain one is loaded, and one
/// wrapped under the app-lock passphrase is [`KeysError::Locked`] (callers that
/// handle the lock use [`load_ark_state`] and [`LockedArk::unlock`]).
///
/// An existing entry must hold exactly 32 bytes; anything else is
/// [`KeysError::Corrupted`] and is left untouched, because replacing it would
/// silently strand data encrypted under the old key.
///
/// A new key is read back before it is returned: a keystore that accepts a write but
/// does not return it intact would lose every byte later encrypted under the key, so
/// that is reported now, before anything depends on it. Key bytes are never logged
/// and never appear in errors.
pub fn load_or_create_ark(
    keystore: &dyn Keystore,
    rng: &dyn Rng,
    install_id: &InstallId,
) -> Result<Ark, KeysError> {
    match load_ark_state(keystore, install_id)? {
        ArkState::Unlocked(ark) => Ok(ark),
        ArkState::Locked(_) => Err(KeysError::Locked),
        ArkState::Missing => lock::create_ark(keystore, rng, install_id),
    }
}

/// Interprets the bytes of an existing keystore entry as an ARK.
pub(crate) fn ark_from_stored(stored: &[u8]) -> Result<Ark, KeysError> {
    secret_from_stored(stored, "root key").map(Ark::from_secret)
}

/// Interprets the bytes of an existing keystore entry as 32 bytes of key material.
/// `what` names the entry in the error; the error never carries the entry's bytes.
pub(crate) fn secret_from_stored(stored: &[u8], what: &str) -> Result<Secret32, KeysError> {
    if stored.len() != ARK_LEN {
        return Err(KeysError::Corrupted(format!(
            "{what} keystore entry holds {} bytes, expected {ARK_LEN}",
            stored.len()
        )));
    }
    // Keep the stack copy wiped; the `Secret32` below owns the only long-lived one.
    let mut raw = Zeroizing::new([0u8; ARK_LEN]);
    raw.copy_from_slice(stored);
    Ok(Secret32::from_bytes(*raw))
}

/// Writes `secret` under `name` and reads it back.
///
/// A keystore that acknowledges the write but does not return the bytes intact gets
/// the entry deleted again (best effort: an entry known to be unusable must not be
/// mistaken for a key later) and the caller gets [`KeysError::Corrupted`]. A failing
/// read is returned as is and leaves the entry in place.
pub(crate) fn store_verified(
    keystore: &dyn Keystore,
    name: &str,
    secret: &[u8],
    what: &str,
) -> Result<(), KeysError> {
    keystore.set(name, secret)?;
    let intact = keystore
        .get(name)?
        .is_some_and(|stored| stored.as_slice() == secret);
    if !intact {
        let _ = keystore.delete(name);
        return Err(KeysError::Corrupted(format!(
            "the keystore did not return the new {what} intact"
        )));
    }
    Ok(())
}

/// `DbKey = HKDF(ARK, Label::LOCAL_DB, info = install_id bytes)`.
#[must_use]
pub fn derive_db_key(ark: &Ark, install_id: &InstallId) -> DbKey {
    DbKey(kb_crypto::derive_key(
        ark.secret(),
        Label::LOCAL_DB,
        install_id.as_bytes(),
    ))
}

/// In-memory keystore for tests and for the web shell's early bring-up.
#[derive(Debug, Default)]
pub struct MemoryKeystore {
    entries: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryKeystore {
    /// Empty keystore.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Keystore for MemoryKeystore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| KeystoreError::Other("poisoned".into()))?;
        Ok(entries.get(name).map(|v| Zeroizing::new(v.clone())))
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| KeystoreError::Other("poisoned".into()))?;
        entries.insert(name.to_owned(), secret.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), KeystoreError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| KeystoreError::Other("poisoned".into()))?;
        entries.remove(name);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use kb_types::testing::SeededRng;

    fn install_id() -> InstallId {
        "12345678-9abc-4def-8123-456789abcdef".parse().unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Keystore that reports "no keystore here" on every read and write.
    struct Unavailable;

    impl Keystore for Unavailable {
        fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
            Err(KeystoreError::Unavailable("no secret service".into()))
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), KeystoreError> {
            Err(KeystoreError::Unavailable("no secret service".into()))
        }
        fn delete(&self, _: &str) -> Result<(), KeystoreError> {
            Ok(())
        }
    }

    /// Keystore that is empty and refuses writes.
    struct ReadOnly;

    impl Keystore for ReadOnly {
        fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), KeystoreError> {
            Err(KeystoreError::Other("read-only".into()))
        }
        fn delete(&self, _: &str) -> Result<(), KeystoreError> {
            Ok(())
        }
    }

    /// Keystore that acknowledges writes and forgets them.
    struct Forgetful;

    impl Keystore for Forgetful {
        fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), KeystoreError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), KeystoreError> {
            Ok(())
        }
    }

    /// Keystore that stores every secret with its last byte flipped (a store that
    /// cannot round-trip binary data).
    #[derive(Default)]
    struct Mangling(MemoryKeystore);

    impl Keystore for Mangling {
        fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
            self.0.get(name)
        }
        fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
            let mut mangled = secret.to_vec();
            if let Some(last) = mangled.last_mut() {
                *last ^= 1;
            }
            self.0.set(name, &mangled)
        }
        fn delete(&self, name: &str) -> Result<(), KeystoreError> {
            self.0.delete(name)
        }
    }

    #[test]
    fn entry_name_is_ark_colon_install_id() {
        assert_eq!(
            ark_entry_name(&install_id()),
            "ark:12345678-9abc-4def-8123-456789abcdef"
        );
    }

    #[test]
    fn create_then_load_returns_the_same_key() {
        let keystore = MemoryKeystore::new();
        let id = install_id();

        let created = load_or_create_ark(&keystore, &SeededRng::new(1), &id).unwrap();
        let stored = keystore.get(&ark_entry_name(&id)).unwrap().unwrap();
        assert_eq!(stored.as_slice(), created.secret().expose());
        assert_eq!(stored.len(), 32);
        assert_ne!(created.secret().expose(), &[0u8; 32]);

        // A different RNG seed proves the second call loaded instead of regenerating.
        let loaded = load_or_create_ark(&keystore, &SeededRng::new(999), &id).unwrap();
        assert_eq!(created, loaded);
        let again = load_or_create_ark(&keystore, &SeededRng::new(5), &id).unwrap();
        assert_eq!(created, again);
    }

    #[test]
    fn a_new_ark_is_drawn_from_the_injected_rng() {
        let keystore = MemoryKeystore::new();
        let ark = load_or_create_ark(&keystore, &SeededRng::new(5), &install_id()).unwrap();
        assert_eq!(
            ark.secret(),
            &Secret32::generate(&SeededRng::new(5)),
            "one 32-byte draw, stored as is"
        );
    }

    #[test]
    fn each_install_has_its_own_entry_and_key() {
        let keystore = MemoryKeystore::new();
        let rng = SeededRng::new(3);
        let id_a = install_id();
        let id_b: InstallId = "12345678-9abc-4def-8123-456789abcdee".parse().unwrap();

        let a = load_or_create_ark(&keystore, &rng, &id_a).unwrap();
        let b = load_or_create_ark(&keystore, &rng, &id_b).unwrap();
        assert_ne!(a, b);
        assert_eq!(load_or_create_ark(&keystore, &rng, &id_a).unwrap(), a);
        assert_eq!(load_or_create_ark(&keystore, &rng, &id_b).unwrap(), b);
        assert!(keystore.get(&ark_entry_name(&id_a)).unwrap().is_some());
        assert!(keystore.get(&ark_entry_name(&id_b)).unwrap().is_some());
    }

    #[test]
    fn wrong_length_entries_are_corrupted_and_left_untouched() {
        let id = install_id();
        let name = ark_entry_name(&id);
        for len in [0usize, 1, 31, 33, 64] {
            let keystore = MemoryKeystore::new();
            let junk = vec![0xAB; len];
            keystore.set(&name, &junk).unwrap();

            let err = load_or_create_ark(&keystore, &SeededRng::new(1), &id).unwrap_err();
            let message = err.to_string();
            assert!(
                matches!(err, KeysError::Corrupted(_)),
                "{len} bytes: {message}"
            );
            assert!(message.contains(&len.to_string()), "length is reported");
            for leaked in ["abab", "0xab", "171"] {
                assert!(!message.contains(leaked), "{leaked} leaked: {message}");
            }

            let after = keystore.get(&name).unwrap().unwrap();
            assert_eq!(
                after.as_slice(),
                junk.as_slice(),
                "entry must not be replaced"
            );
        }
    }

    #[test]
    fn keystore_failures_propagate() {
        let id = install_id();
        let rng = SeededRng::new(1);

        let err = load_or_create_ark(&Unavailable, &rng, &id).unwrap_err();
        assert!(
            matches!(
                &err,
                KeysError::Keystore(KeystoreError::Unavailable(m)) if m == "no secret service"
            ),
            "{err:?}"
        );
        let err = load_or_create_ark(&ReadOnly, &rng, &id).unwrap_err();
        assert!(
            matches!(
                &err,
                KeysError::Keystore(KeystoreError::Other(m)) if m == "read-only"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_keystore_that_cannot_return_the_key_is_detected() {
        let id = install_id();
        let rng = SeededRng::new(1);

        let err = load_or_create_ark(&Forgetful, &rng, &id).unwrap_err();
        assert!(matches!(err, KeysError::Corrupted(_)), "{err}");

        let mangling = Mangling::default();
        let err = load_or_create_ark(&mangling, &rng, &id).unwrap_err();
        assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
        assert!(
            mangling.get(&ark_entry_name(&id)).unwrap().is_none(),
            "the unusable entry is removed again"
        );
    }

    #[test]
    fn derive_db_key_golden_vector() {
        // Frozen for HKDF label `atomik-kb/v1/local-db`: if this changes, every
        // existing database becomes unreadable. The expected value was cross-checked
        // against Python's hmac and OpenSSL's HKDF (see fixtures/crypto/).
        let mut ark_bytes = [0u8; 32];
        for (i, byte) in (0u8..).zip(ark_bytes.iter_mut()) {
            *byte = i.wrapping_mul(5).wrapping_add(1);
        }
        let ark = Ark::from_secret(Secret32::from_bytes(ark_bytes));
        assert_eq!(
            hex(ark.secret().expose()),
            "01060b10151a1f24292e33383d42474c51565b60656a6f74797e83888d92979c"
        );

        let key = derive_db_key(&ark, &install_id());
        assert_eq!(
            hex(key.secret().expose()),
            "b38c4b2aa6ed504b429e35ae1eaacb9e49caf48c0fe38782a56eefe490321119"
        );

        // The info is the install id's 16 bytes, not its text form.
        let other: InstallId = "12345678-9abc-4def-8123-456789abcdee".parse().unwrap();
        assert_eq!(
            hex(derive_db_key(&ark, &other).secret().expose()),
            "728377ec1ea34e4f1c20b6a66c47280cd30e94ee61ac7d1d97dbe0c50aa9bbc6"
        );
    }

    #[test]
    fn db_key_is_bound_to_the_ark_and_the_install_and_is_not_the_ark() {
        let id = install_id();
        let ark = Ark::from_secret(Secret32::generate(&SeededRng::new(1)));
        let other_ark = Ark::from_secret(Secret32::generate(&SeededRng::new(2)));
        let key = derive_db_key(&ark, &id);

        assert_eq!(key, derive_db_key(&ark, &id), "deterministic");
        assert_ne!(key, derive_db_key(&other_ark, &id));
        assert_ne!(
            key,
            derive_db_key(&ark, &InstallId::from_bytes([9; 16])),
            "another installation derives another key"
        );
        assert_ne!(key.secret(), ark.secret(), "the DB key is not the ARK");
    }

    #[test]
    fn debug_output_never_contains_key_bytes() {
        let ark = Ark::from_secret(Secret32::from_bytes([0xAB; 32]));
        let key = derive_db_key(&ark, &install_id());
        assert_eq!(format!("{ark:?}"), "Ark(Secret32(<redacted>))");
        assert_eq!(format!("{key:?}"), "DbKey(Secret32(<redacted>))");
    }

    #[test]
    fn memory_keystore_semantics() {
        let keystore = MemoryKeystore::new();
        assert!(keystore.get("a").unwrap().is_none());
        keystore.delete("a").unwrap();

        keystore.set("a", b"one").unwrap();
        keystore.set("b", b"two").unwrap();
        assert_eq!(keystore.get("a").unwrap().unwrap().as_slice(), b"one");
        keystore.set("a", b"three").unwrap();
        assert_eq!(keystore.get("a").unwrap().unwrap().as_slice(), b"three");
        assert_eq!(keystore.get("b").unwrap().unwrap().as_slice(), b"two");

        keystore.delete("a").unwrap();
        keystore.delete("a").unwrap();
        assert!(keystore.get("a").unwrap().is_none());
        assert!(keystore.get("b").unwrap().is_some());
    }
}
