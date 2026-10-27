//! The OS keystore behind [`kb_keys::Keystore`].
//!
//! Built on the keyring ecosystem (`keyring` 4.x): [`keyring_core`] is the API, and
//! each platform contributes one credential-store crate:
//!
//! | Platform | Store | Notes |
//! |---|---|---|
//! | Windows | `windows-native-keyring-store` (Credential Manager) | entries are written with **Local** persistence |
//! | macOS | `apple-native-keyring-store`, `keychain` module | the login keychain |
//! | Linux, other Unix | `zbus-secret-service-keyring-store` | needs a running Secret Service (GNOME Keyring, KWallet) |
//!
//! No global default store is involved: every [`NativeKeystore`] owns its store and
//! builds entries from it, so two keystores (or a test double) never interfere.
//!
//! Stored representation, frozen as v1: the secret as **lowercase hex text**, written
//! with `set_password`. Every store round-trips text, whereas the KDE Wallet behind the
//! Secret Service only holds UTF-8 and would mangle raw key bytes. Secrets here are
//! 32-byte keys, so the cost is a 64-character value.

use std::collections::HashMap;
use std::sync::Arc;

use kb_keys::{Keystore, KeystoreError};
use keyring_core::{CredentialStore, Entry, Error as KeyringError};
use zeroize::{Zeroize, Zeroizing};

/// Entry modifiers applied to every credential this keystore builds.
type Modifiers = HashMap<&'static str, &'static str>;

/// OS keystore through the keyring ecosystem.
///
/// Windows: Credential Manager with Local persistence (never roaming). macOS: Keychain.
/// Linux: secret service; returns [`KeystoreError::Unavailable`] when none is running.
#[derive(Debug)]
pub struct NativeKeystore {
    service: String,
    store: Arc<CredentialStore>,
    modifiers: Modifiers,
}

impl NativeKeystore {
    /// Service name under which entries are stored, e.g. `"atomik-kb"`.
    ///
    /// Opens the platform's credential store. That does not read or write any secret,
    /// but on Linux it connects to the Secret Service, so a machine without one fails
    /// here with [`KeystoreError::Unavailable`].
    pub fn new(service: impl Into<String>) -> Result<Self, KeystoreError> {
        let (store, modifiers) = platform::open_store()?;
        tracing::debug!(vendor = %store.vendor(), "native keystore ready");
        Ok(Self::with_store(service, store, modifiers))
    }

    fn with_store(
        service: impl Into<String>,
        store: Arc<CredentialStore>,
        modifiers: Modifiers,
    ) -> Self {
        Self {
            service: service.into(),
            store,
            modifiers,
        }
    }

    /// The credential for `name`. Building an entry does not touch the store.
    fn entry(&self, name: &str) -> Result<Entry, KeystoreError> {
        let modifiers = (!self.modifiers.is_empty()).then_some(&self.modifiers);
        self.store
            .build(&self.service, name, modifiers)
            .map_err(map_error)
    }
}

impl Keystore for NativeKeystore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        let text = match self.entry(name)?.get_password() {
            Ok(text) => Zeroizing::new(text),
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(err) => return Err(map_error(err)),
        };
        decode(&text).map(Some).ok_or_else(|| {
            // Never replace or guess: an entry we did not write is the user's to resolve.
            KeystoreError::Other("the stored value is not in the expected format".to_owned())
        })
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        let text = encode(secret);
        self.entry(name)?.set_password(&text).map_err(map_error)
    }

    fn delete(&self, name: &str) -> Result<(), KeystoreError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(err) => Err(map_error(err)),
        }
    }
}

/// Lowercase hex, in a buffer that is wiped on drop.
fn encode(secret: &[u8]) -> Zeroizing<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = Zeroizing::new(String::with_capacity(2 * secret.len()));
    for byte in secret {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

/// Inverse of [`encode`]; accepts either case. `None` for anything that is not an even
/// number of hex digits.
fn decode(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    let (pairs, odd_digit) = text.as_bytes().as_chunks::<2>();
    if !odd_digit.is_empty() {
        return None;
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(pairs.len()));
    for [high, low] in pairs {
        let high = char::from(*high).to_digit(16)?;
        let low = char::from(*low).to_digit(16)?;
        bytes.push(u8::try_from((high << 4) | low).ok()?);
    }
    Some(bytes)
}

/// Maps a keyring error to the keystore contract. Messages carry the store's own
/// description of the failure and never a secret (the raw bytes inside a few keyring
/// error variants are wiped and left out).
fn map_error(err: KeyringError) -> KeystoreError {
    match err {
        // Locked, no logon session, access denied: there is a keystore, but not for us now.
        KeyringError::NoStorageAccess(cause) => {
            KeystoreError::Unavailable(format!("the keystore cannot be accessed: {cause}"))
        }
        KeyringError::BadEncoding(mut bytes) => {
            bytes.zeroize();
            KeystoreError::Other("the stored value is not valid UTF-8".to_owned())
        }
        KeyringError::BadDataFormat(mut bytes, cause) => {
            bytes.zeroize();
            KeystoreError::Other(format!("the stored value is malformed: {cause}"))
        }
        other => KeystoreError::Other(format!("keystore error: {other}")),
    }
}

/// Windows: Credential Manager.
#[cfg(windows)]
mod platform {
    use super::{Arc, CredentialStore, KeystoreError, Modifiers};

    pub(super) fn open_store() -> Result<(Arc<CredentialStore>, Modifiers), KeystoreError> {
        let store: Arc<CredentialStore> =
            windows_native_keyring_store::Store::new().map_err(|err| {
                KeystoreError::Unavailable(format!("cannot open the Credential Manager: {err}"))
            })?;
        // The store's default is Enterprise persistence (`CRED_PERSIST_ENTERPRISE`):
        // such credentials roam to the user's other machines in a domain. The ARK must
        // stay on this computer, so ask for Local (`CRED_PERSIST_LOCAL_MACHINE`). The
        // setting is per entry, applied whenever the secret is written.
        Ok((store, Modifiers::from([("persistence", "Local")])))
    }
}

/// macOS: the login keychain.
#[cfg(target_os = "macos")]
mod platform {
    use super::{Arc, CredentialStore, KeystoreError, Modifiers};

    pub(super) fn open_store() -> Result<(Arc<CredentialStore>, Modifiers), KeystoreError> {
        let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new()
            .map_err(|err| {
                KeystoreError::Unavailable(format!("cannot open the keychain: {err}"))
            })?;
        Ok((store, Modifiers::new()))
    }
}

/// Linux and other Unixes: the freedesktop Secret Service over D-Bus.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod platform {
    use super::{Arc, CredentialStore, KeystoreError, Modifiers};

    pub(super) fn open_store() -> Result<(Arc<CredentialStore>, Modifiers), KeystoreError> {
        // Creating the store connects to the Secret Service, so a failure here means
        // there is none to talk to (no session bus, GNOME Keyring or KWallet not running).
        // There is no passphrase fallback in this milestone (it arrives with the app lock).
        let store: Arc<CredentialStore> =
            zbus_secret_service_keyring_store::Store::new().map_err(|err| {
                KeystoreError::Unavailable(format!("no secret service is available: {err}"))
            })?;
        Ok((store, Modifiers::new()))
    }
}

/// Platforms without a native keystore adapter yet (Android and iOS get theirs later).
#[cfg(not(any(
    windows,
    target_os = "macos",
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )
)))]
mod platform {
    use super::{Arc, CredentialStore, KeystoreError, Modifiers};

    pub(super) fn open_store() -> Result<(Arc<CredentialStore>, Modifiers), KeystoreError> {
        Err(KeystoreError::Unavailable(
            "this platform has no native keystore adapter yet".to_owned(),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Write;

    use super::*;
    use keyring_core::api::CredentialStoreApi;
    use keyring_core::mock;

    const SERVICE: &str = "atomik-kb-unit-test";

    /// A keystore on keyring-core's in-memory mock store, plus the store for peeking
    /// at the stored representation and injecting failures.
    fn mock_keystore() -> (NativeKeystore, Arc<mock::Store>) {
        let store = mock::Store::new().unwrap();
        let keystore = NativeKeystore::with_store(SERVICE, store.clone(), Modifiers::new());
        (keystore, store)
    }

    /// What the store holds for `name`, as the text it was written as.
    fn stored_text(store: &Arc<mock::Store>, name: &str) -> Option<String> {
        store
            .build(SERVICE, name, None)
            .unwrap()
            .get_password()
            .ok()
    }

    /// Makes the next operation on `name` fail with `err`.
    fn inject(store: &Arc<mock::Store>, name: &str, err: KeyringError) {
        let entry = store.build(SERVICE, name, None).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(err);
    }

    fn platform_error(text: &str) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(std::io::Error::other(text.to_owned()))
    }

    #[test]
    fn hex_roundtrips_every_byte_value() {
        let all: Vec<u8> = (0..=255).collect();
        let text = encode(&all);
        assert_eq!(text.len(), 512);
        assert!(text.starts_with("000102"));
        assert!(text.ends_with("fdfeff"));
        assert_eq!(decode(&text).unwrap().as_slice(), all.as_slice());
        assert_eq!(decode("").unwrap().as_slice(), b"");
    }

    #[test]
    fn hex_decoding_is_strict_about_shape_and_lenient_about_case() {
        assert_eq!(decode("00ABff").unwrap().as_slice(), [0x00, 0xab, 0xff]);
        for bad in ["0", "abc", "zz", "0g", " 00", "00 ", "+1", "é1", "0x00"] {
            assert!(decode(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn set_get_delete_roundtrip_with_arbitrary_bytes() {
        let (keystore, _) = mock_keystore();
        let secret: Vec<u8> = (0..=255).rev().collect();
        keystore.set("ark:one", &secret).unwrap();
        assert_eq!(
            keystore.get("ark:one").unwrap().unwrap().as_slice(),
            secret.as_slice()
        );

        keystore.set("ark:one", &[1, 2, 3]).unwrap();
        assert_eq!(
            keystore.get("ark:one").unwrap().unwrap().as_slice(),
            [1, 2, 3]
        );

        keystore.delete("ark:one").unwrap();
        assert!(keystore.get("ark:one").unwrap().is_none());
    }

    #[test]
    fn missing_entry_reads_as_none_and_deleting_it_succeeds() {
        let (keystore, _) = mock_keystore();
        assert!(keystore.get("ark:missing").unwrap().is_none());
        keystore.delete("ark:missing").unwrap();
        keystore.delete("ark:missing").unwrap();
    }

    #[test]
    fn secrets_are_stored_as_lowercase_hex_text() {
        // The stored form is part of the v1 format: changing it strands existing keys.
        let (keystore, store) = mock_keystore();
        keystore.set("ark:x", &[0x00, 0xab, 0xff, 0x10]).unwrap();
        assert_eq!(stored_text(&store, "ark:x").as_deref(), Some("00abff10"));
    }

    #[test]
    fn entries_are_namespaced_by_service_and_name() {
        let store = mock::Store::new().unwrap();
        let a = NativeKeystore::with_store("service-a", store.clone(), Modifiers::new());
        let b = NativeKeystore::with_store("service-b", store, Modifiers::new());
        a.set("ark:x", b"A").unwrap();
        assert!(b.get("ark:x").unwrap().is_none());
        b.set("ark:x", b"B").unwrap();
        assert_eq!(a.get("ark:x").unwrap().unwrap().as_slice(), b"A");
        assert_eq!(b.get("ark:x").unwrap().unwrap().as_slice(), b"B");
        a.set("ark:y", b"Y").unwrap();
        assert_eq!(a.get("ark:x").unwrap().unwrap().as_slice(), b"A");
    }

    #[test]
    fn values_that_were_not_written_by_us_are_reported_not_guessed() {
        let (keystore, store) = mock_keystore();
        for foreign in ["not hex at all", "abc", "SECRET-VALUE-zz"] {
            store
                .build(SERVICE, "ark:foreign", None)
                .unwrap()
                .set_password(foreign)
                .unwrap();
            let err = keystore.get("ark:foreign").unwrap_err();
            assert!(matches!(err, KeystoreError::Other(_)), "{err}");
            assert!(
                !err.to_string().contains(foreign),
                "the value must not leak: {err}"
            );
            assert_eq!(
                stored_text(&store, "ark:foreign").as_deref(),
                Some(foreign),
                "reading never rewrites the entry"
            );
        }
    }

    #[test]
    fn inaccessible_keystore_maps_to_unavailable() {
        let (keystore, store) = mock_keystore();
        keystore.set("ark:x", b"v").unwrap();

        inject(
            &store,
            "ark:x",
            KeyringError::NoStorageAccess(platform_error("locked")),
        );
        let err = keystore.get("ark:x").unwrap_err();
        assert!(
            matches!(&err, KeystoreError::Unavailable(m) if m.contains("locked")),
            "{err:?}"
        );

        inject(
            &store,
            "ark:x",
            KeyringError::NoStorageAccess(platform_error("locked")),
        );
        assert!(matches!(
            keystore.set("ark:x", b"w").unwrap_err(),
            KeystoreError::Unavailable(_)
        ));

        inject(
            &store,
            "ark:x",
            KeyringError::NoStorageAccess(platform_error("locked")),
        );
        assert!(matches!(
            keystore.delete("ark:x").unwrap_err(),
            KeystoreError::Unavailable(_)
        ));

        // Failures leave the stored value alone, and the next call works again.
        assert_eq!(keystore.get("ark:x").unwrap().unwrap().as_slice(), b"v");
    }

    #[test]
    fn other_failures_map_to_other_and_never_echo_secret_bytes() {
        let (keystore, store) = mock_keystore();

        inject(
            &store,
            "ark:x",
            KeyringError::PlatformFailure(platform_error("disk on fire")),
        );
        let err = keystore.set("ark:x", b"v").unwrap_err();
        assert!(
            matches!(&err, KeystoreError::Other(m) if m.contains("disk on fire")),
            "{err:?}"
        );

        inject(
            &store,
            "ark:x",
            KeyringError::BadEncoding(b"SECRET-BYTES".to_vec()),
        );
        let err = keystore.get("ark:x").unwrap_err();
        assert!(matches!(err, KeystoreError::Other(_)), "{err}");
        assert!(!err.to_string().contains("SECRET-BYTES"), "{err}");

        inject(
            &store,
            "ark:x",
            KeyringError::BadDataFormat(b"SECRET-BYTES".to_vec(), platform_error("garbled")),
        );
        let err = keystore.get("ark:x").unwrap_err();
        assert!(
            matches!(&err, KeystoreError::Other(m) if m.contains("garbled")),
            "{err:?}"
        );
        assert!(!err.to_string().contains("SECRET-BYTES"), "{err}");

        inject(
            &store,
            "ark:x",
            KeyringError::TooLong("secret".to_owned(), 10),
        );
        assert!(matches!(
            keystore.set("ark:x", b"v").unwrap_err(),
            KeystoreError::Other(_)
        ));
    }

    #[test]
    fn debug_output_names_the_service_but_holds_no_secret() {
        let (keystore, _) = mock_keystore();
        keystore.set("ark:x", &[0xde, 0xad, 0xbe, 0xef]).unwrap();
        let debug = format!("{keystore:?}");
        assert!(debug.contains(SERVICE), "{debug}");
        assert!(!debug.contains("deadbeef"), "{debug}");
    }

    /// Opt-in: this touches the machine's real credential store (Windows Credential
    /// Manager, macOS Keychain, Secret Service). Run it with
    /// `KB_TEST_KEYSTORE=1 cargo test -p kb-storage-native real_keystore_roundtrip`.
    /// It uses its own service name and a unique entry name, and deletes the entry
    /// again, also when an assertion fails.
    #[test]
    fn real_keystore_roundtrip() {
        if std::env::var("KB_TEST_KEYSTORE").as_deref() != Ok("1") {
            return;
        }

        /// Deletes the test entry on the way out, even when an assertion fails.
        struct Cleanup<'a>(&'a NativeKeystore, String);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.delete(&self.1);
            }
        }

        let keystore = NativeKeystore::new("atomik-kb-test").unwrap();
        // Visible with `--nocapture`: which store this ran against (no secrets in it).
        let _ = writeln!(std::io::stderr(), "real_keystore_roundtrip: {keystore:?}");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("ark:test-{}-{nanos}", std::process::id());
        let _cleanup = Cleanup(&keystore, name.clone());

        // Absent to begin with; deleting an absent entry succeeds.
        assert!(keystore.get(&name).unwrap().is_none());
        keystore.delete(&name).unwrap();

        // A 32-byte key and every byte value survive the round trip.
        let key: Vec<u8> = (0u8..32)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        keystore.set(&name, &key).unwrap();
        assert_eq!(
            keystore.get(&name).unwrap().unwrap().as_slice(),
            key.as_slice()
        );
        let all: Vec<u8> = (0..=255).collect();
        keystore.set(&name, &all).unwrap();
        assert_eq!(
            keystore.get(&name).unwrap().unwrap().as_slice(),
            all.as_slice()
        );

        // Windows: the credential really is stored with Local persistence.
        #[cfg(windows)]
        {
            let attributes = keystore.entry(&name).unwrap().get_attributes().unwrap();
            assert_eq!(
                attributes.get("persistence").map(String::as_str),
                Some("Local"),
                "attributes: {attributes:?}"
            );
        }

        // Replace, then delete; afterwards it is gone and deleting again is fine.
        keystore.set(&name, &key).unwrap();
        assert_eq!(
            keystore.get(&name).unwrap().unwrap().as_slice(),
            key.as_slice()
        );
        keystore.delete(&name).unwrap();
        assert!(keystore.get(&name).unwrap().is_none());
        keystore.delete(&name).unwrap();
    }
}
