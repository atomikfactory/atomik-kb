//! The `Keystore` of the core over the callback the app implements (Android `AndroidKeyStore`-wrapped files).

use std::sync::Arc;

use kb_keys::{Keystore, KeystoreError};
use zeroize::Zeroizing;

/// Errors the app's keystore reports.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KeystoreFfiError {
    /// No usable keystore (the key is gone, the device is locked).
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The keystore exists but the operation failed.
    #[error("failed: {0}")]
    Failed(String),
}

impl From<uniffi::UnexpectedUniFFICallbackError> for KeystoreFfiError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed(error.reason)
    }
}

/// Platform secret storage, implemented by the app. Values are small (tens of bytes); names are stable strings.
#[uniffi::export(with_foreign)]
pub trait PlatformKeystore: Send + Sync {
    /// Reads a secret; `None` when absent.
    fn get(&self, name: String) -> Result<Option<Vec<u8>>, KeystoreFfiError>;
    /// Creates or replaces a secret.
    fn set(&self, name: String, value: Vec<u8>) -> Result<(), KeystoreFfiError>;
    /// Deletes a secret; succeeds when absent.
    fn delete(&self, name: String) -> Result<(), KeystoreFfiError>;
}

fn map_error(error: KeystoreFfiError) -> KeystoreError {
    match error {
        KeystoreFfiError::Unavailable(message) => KeystoreError::Unavailable(message),
        KeystoreFfiError::Failed(message) => KeystoreError::Other(message),
    }
}

/// [`kb_keys::Keystore`] on a [`PlatformKeystore`]. A panic of the callback is a keystore failure, not a crash.
pub(crate) struct KeystoreAdapter(pub(crate) Arc<dyn PlatformKeystore>);

fn guarded<T>(call: impl FnOnce() -> Result<T, KeystoreFfiError>) -> Result<T, KeystoreError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(result) => result.map_err(map_error),
        Err(_) => Err(KeystoreError::Other(
            "the keystore callback panicked".to_owned(),
        )),
    }
}

impl Keystore for KeystoreAdapter {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        guarded(|| self.0.get(name.to_owned())).map(|value| value.map(Zeroizing::new))
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        guarded(|| self.0.set(name.to_owned(), secret.to_vec()))
    }

    fn delete(&self, name: &str) -> Result<(), KeystoreError> {
        guarded(|| self.0.delete(name.to_owned()))
    }
}
