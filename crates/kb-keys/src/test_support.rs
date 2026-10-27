//! Test doubles and fixtures shared by the unit tests of this crate.

#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use kb_crypto::{Argon2Params, Secret32};
use kb_types::testing::SeededRng;
use kb_types::{DeviceId, InstallId};
use zeroize::Zeroizing;

use crate::device::DeviceKeys;
use crate::{Ark, Keystore, KeystoreError, MemoryKeystore};

/// Argon2id parameters small enough to keep tests fast (the smallest the algorithm
/// allows: 8 KiB, one pass, one lane). Only ever used with an explicit minimum.
pub(crate) const TINY: Argon2Params = Argon2Params {
    memory_kib: 8,
    iterations: 1,
    parallelism: 1,
};

/// A fixed installation.
pub(crate) fn install_id() -> InstallId {
    "12345678-9abc-4def-8123-456789abcdef".parse().unwrap()
}

/// Another installation.
pub(crate) fn other_install_id() -> InstallId {
    "12345678-9abc-4def-8123-456789abcdee".parse().unwrap()
}

/// A deterministic ARK.
pub(crate) fn ark(seed: u64) -> Ark {
    Ark::from_secret(Secret32::generate(&SeededRng::new(seed)))
}

/// A deterministic device: id and keys.
pub(crate) fn device(seed: u64) -> (DeviceId, DeviceKeys) {
    let rng = SeededRng::new(seed);
    (DeviceId::generate(&rng), DeviceKeys::generate(&rng))
}

/// A memory keystore with injectable faults that records every operation.
#[derive(Default)]
pub(crate) struct Faulty {
    /// Backing store.
    pub(crate) inner: MemoryKeystore,
    /// Writes to entries whose name starts with this prefix fail.
    pub(crate) fail_set: Option<&'static str>,
    /// Writes to entries whose name starts with this prefix are stored with their last
    /// byte flipped (a store that cannot round-trip binary data).
    pub(crate) mangle_set: Option<&'static str>,
    /// Every delete fails.
    pub(crate) fail_delete: bool,
    /// Operations in order: `get:<name>`, `set:<name>`, `delete:<name>`.
    pub(crate) ops: Mutex<Vec<String>>,
}

impl Faulty {
    /// Stores every write with its last byte flipped.
    pub(crate) fn mangling() -> Self {
        Self {
            mangle_set: Some(""),
            ..Self::default()
        }
    }

    /// The operations so far.
    pub(crate) fn ops(&self) -> Vec<String> {
        self.ops.lock().unwrap().clone()
    }

    /// Forgets the recorded operations.
    pub(crate) fn clear_ops(&self) {
        self.ops.lock().unwrap().clear();
    }

    fn record(&self, op: &str, name: &str) {
        self.ops.lock().unwrap().push(format!("{op}:{name}"));
    }
}

impl Keystore for Faulty {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        self.record("get", name);
        self.inner.get(name)
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        self.record("set", name);
        if self.fail_set.is_some_and(|prefix| name.starts_with(prefix)) {
            return Err(KeystoreError::Other("write refused".into()));
        }
        if self
            .mangle_set
            .is_some_and(|prefix| name.starts_with(prefix))
        {
            let mut mangled = secret.to_vec();
            if let Some(last) = mangled.last_mut() {
                *last ^= 1;
            }
            return self.inner.set(name, &mangled);
        }
        self.inner.set(name, secret)
    }

    fn delete(&self, name: &str) -> Result<(), KeystoreError> {
        self.record("delete", name);
        if self.fail_delete {
            return Err(KeystoreError::Other("delete refused".into()));
        }
        self.inner.delete(name)
    }
}

/// Keystore that acknowledges writes and forgets them.
pub(crate) struct Forgetful;

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

/// Keystore that reports "no keystore here" on every operation.
pub(crate) struct Unavailable;

impl Keystore for Unavailable {
    fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        Err(KeystoreError::Unavailable("no secret service".into()))
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), KeystoreError> {
        Err(KeystoreError::Unavailable("no secret service".into()))
    }
    fn delete(&self, _: &str) -> Result<(), KeystoreError> {
        Err(KeystoreError::Unavailable("no secret service".into()))
    }
}
