//! The app lock (ADR-0018): the keystore holds either the plain ARK (`ark:<install>`)
//! or the ARK wrapped under an Argon2id-derived key (`ark-locked:<install>`).
//!
//! The locked entry is [`PassphraseWrapped::to_bytes`] of
//! `wrap_with_passphrase(ARK, passphrase, params, subject = "ark-locked" || install id
//! bytes)`: the Argon2id parameters and salt travel with the wrapped key, and the
//! subject binds the blob to its installation. Transitions write the new entry, read
//! it back, then delete the other one, so an interrupted transition leaves both; the
//! next [`load_ark_state`] resolves that in favour of the locked entry.

use kb_crypto::{Argon2Params, CryptoError, PassphraseWrapped, Secret32};
use kb_types::{InstallId, Rng};

use crate::{Ark, KeysError, Keystore, ark_entry_name, ark_from_stored, store_verified};

/// Shortest accepted passphrase, in bytes.
pub const PASSPHRASE_MIN_BYTES: usize = 8;
/// Longest accepted passphrase, in bytes.
pub const PASSPHRASE_MAX_BYTES: usize = 256;

/// First part of the AAD subject of the locked ARK; the install id bytes follow.
const LOCKED_SUBJECT_PREFIX: &[u8] = b"ark-locked";

/// Keystore entry holding the passphrase-wrapped ARK.
#[must_use]
pub fn lock_entry_name(install_id: &InstallId) -> String {
    format!("ark-locked:{install_id}")
}

/// `ark-locked || install id bytes`.
fn locked_subject(install_id: &InstallId) -> Vec<u8> {
    [LOCKED_SUBJECT_PREFIX, install_id.as_bytes()].concat()
}

/// A wrapped ARK waiting for its passphrase. Holds no key material.
///
/// Its `Debug` output shows the installation and the Argon2id parameters only: the
/// salt and the wrapped key would let whoever reads a log guess the passphrase offline.
#[derive(Clone)]
pub struct LockedArk {
    blob: PassphraseWrapped,
    install_id: InstallId,
}

impl LockedArk {
    /// From a stored blob.
    #[must_use]
    pub fn new(blob: PassphraseWrapped, install_id: InstallId) -> Self {
        Self { blob, install_id }
    }

    /// The Argon2id parameters the blob was made with.
    #[must_use]
    pub fn params(&self) -> Argon2Params {
        self.blob.params
    }

    /// Unwraps the ARK; a wrong passphrase is [`KeysError::WrongPassphrase`] (after the
    /// full Argon2id cost), weaker-than-minimum parameters [`KeysError::Crypto`].
    ///
    /// The minimum is [`Argon2Params::MINIMUM`]; see [`Self::unlock_with_minimum`].
    pub fn unlock(&self, passphrase: &str) -> Result<Ark, KeysError> {
        self.unlock_with_minimum(passphrase, &Argon2Params::MINIMUM)
    }

    /// [`Self::unlock`] with the minimum Argon2id parameters the caller accepts: the
    /// core passes the parameters its shell configured (production shells
    /// [`Argon2Params::DEFAULT`], tests small ones so Argon2id stays fast). A blob made
    /// with weaker parameters than `minimum` is [`KeysError::Crypto`]
    /// (`WeakParameters`), refused before any Argon2id work; a wrong passphrase is
    /// [`KeysError::WrongPassphrase`].
    pub fn unlock_with_minimum(
        &self,
        passphrase: &str,
        minimum: &Argon2Params,
    ) -> Result<Ark, KeysError> {
        match kb_crypto::unwrap_with_passphrase(
            &self.blob,
            passphrase.as_bytes(),
            &locked_subject(&self.install_id),
            minimum,
        ) {
            Ok(secret) => Ok(Ark::from_secret(secret)),
            Err(CryptoError::Authentication) => Err(KeysError::WrongPassphrase),
            Err(other) => Err(KeysError::Crypto(other)),
        }
    }
}

impl core::fmt::Debug for LockedArk {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LockedArk")
            .field("install_id", &self.install_id)
            .field("params", &self.blob.params)
            .finish_non_exhaustive()
    }
}

/// What the keystore holds for this installation.
#[derive(Debug)]
pub enum ArkState {
    /// The plain ARK (no lock).
    Unlocked(Ark),
    /// A passphrase-wrapped ARK.
    Locked(LockedArk),
    /// Nothing yet (first launch).
    Missing,
}

/// Reads the keystore. When both entries exist (a crash between the two writes of a
/// transition) the locked one wins and the plain one is deleted.
///
/// A malformed entry is [`KeysError::Corrupted`] and nothing is deleted (a malformed
/// locked entry never displaces a plain one). Keystore failures, including a failed
/// delete of the superseded plain entry, are returned as they are.
pub fn load_ark_state(
    keystore: &dyn Keystore,
    install_id: &InstallId,
) -> Result<ArkState, KeysError> {
    let plain_name = ark_entry_name(install_id);
    if let Some(stored) = keystore.get(&lock_entry_name(install_id))? {
        let blob = PassphraseWrapped::from_bytes(&stored).map_err(|_| malformed_locked_entry())?;
        if keystore.get(&plain_name)?.is_some() {
            keystore.delete(&plain_name)?;
            tracing::warn!(
                "both root key entries were present (interrupted lock change); kept the locked one"
            );
        }
        return Ok(ArkState::Locked(LockedArk::new(blob, *install_id)));
    }
    match keystore.get(&plain_name)? {
        Some(stored) => ark_from_stored(&stored).map(ArkState::Unlocked),
        None => Ok(ArkState::Missing),
    }
}

/// Creates a fresh ARK and stores it unlocked (read back before use).
///
/// One 32-byte draw from `rng`. If the keystore already holds a root key for this
/// installation (plain or locked) nothing is written and the result is
/// [`KeysError::Corrupted`]: overwriting it would strand everything encrypted under
/// it. A keystore that does not return the new key intact is [`KeysError::Corrupted`]
/// and the entry is removed again.
pub fn create_ark(
    keystore: &dyn Keystore,
    rng: &dyn Rng,
    install_id: &InstallId,
) -> Result<Ark, KeysError> {
    let plain_name = ark_entry_name(install_id);
    if keystore.get(&lock_entry_name(install_id))?.is_some() || keystore.get(&plain_name)?.is_some()
    {
        return Err(KeysError::Corrupted(
            "the keystore already holds a root key for this installation".to_owned(),
        ));
    }
    let ark = Ark::from_secret(Secret32::generate(rng));
    store_verified(keystore, &plain_name, ark.secret().expose(), "root key")?;
    tracing::info!("created a new account root key in the keystore");
    Ok(ark)
}

/// Validates a passphrase: 8 to 256 bytes (UTF-8 bytes, not characters);
/// [`KeysError::PassphraseLength`] otherwise.
pub fn check_passphrase(passphrase: &str) -> Result<(), KeysError> {
    if (PASSPHRASE_MIN_BYTES..=PASSPHRASE_MAX_BYTES).contains(&passphrase.len()) {
        Ok(())
    } else {
        Err(KeysError::PassphraseLength)
    }
}

/// Writes the ARK as the keystore should hold it: wrapped under `lock` when given
/// (locked entry written first, plain entry deleted), plain otherwise (plain written
/// first, locked entry deleted). Reads back before deleting the other entry.
///
/// With a lock, the passphrase is checked first ([`check_passphrase`]) and the blob is
/// made with exactly the given parameters: pass at least the minimum that
/// [`LockedArk::unlock_with_minimum`] will be called with, or the blob cannot be
/// unlocked. Both paths replace an existing entry of their kind (root key rotation
/// stores the new ARK this way). A write that does not read back intact is
/// [`KeysError::Corrupted`]; the unusable entry is removed and the other entry is left
/// in place.
pub fn store_ark(
    keystore: &dyn Keystore,
    install_id: &InstallId,
    ark: &Ark,
    lock: Option<(&str, Argon2Params)>,
    rng: &dyn Rng,
) -> Result<(), KeysError> {
    let plain_name = ark_entry_name(install_id);
    let locked_name = lock_entry_name(install_id);
    match lock {
        Some((passphrase, params)) => {
            check_passphrase(passphrase)?;
            let blob = kb_crypto::wrap_with_passphrase(
                ark.secret(),
                passphrase.as_bytes(),
                params,
                &locked_subject(install_id),
                rng,
            )?;
            store_verified(keystore, &locked_name, &blob.to_bytes(), "locked root key")?;
            keystore.delete(&plain_name)?;
        }
        None => {
            store_verified(keystore, &plain_name, ark.secret().expose(), "root key")?;
            keystore.delete(&locked_name)?;
        }
    }
    Ok(())
}

/// Enables the lock: [`store_ark`] with a passphrase.
///
/// An existing locked entry is replaced without asking for its passphrase; the core
/// enables the lock only while it is off (see [`change_passphrase`] otherwise).
pub fn enable_lock(
    keystore: &dyn Keystore,
    install_id: &InstallId,
    ark: &Ark,
    passphrase: &str,
    params: Argon2Params,
    rng: &dyn Rng,
) -> Result<(), KeysError> {
    store_ark(keystore, install_id, ark, Some((passphrase, params)), rng)
}

/// Disables the lock: [`store_ark`] without a passphrase.
///
/// No passphrase is checked here; the core verifies it first (for example with
/// [`load_ark_state`] and [`LockedArk::unlock_with_minimum`]).
pub fn disable_lock(
    keystore: &dyn Keystore,
    install_id: &InstallId,
    ark: &Ark,
    rng: &dyn Rng,
) -> Result<(), KeysError> {
    store_ark(keystore, install_id, ark, None, rng)
}

/// Changes the passphrase: verifies `current` against the stored blob, then
/// [`enable_lock`] with `new`.
///
/// `new` is checked first ([`KeysError::PassphraseLength`], before any Argon2id work).
/// `current` is verified with `params` as the minimum, so a stored blob weaker than
/// `params` is [`KeysError::Crypto`] (`WeakParameters`). A wrong `current`, or no
/// locked entry at all (the lock is off, so no passphrase can match), is
/// [`KeysError::WrongPassphrase`]; a stored blob that wraps another ARK than `ark` is
/// [`KeysError::Corrupted`]. Nothing is written unless every check passes.
pub fn change_passphrase(
    keystore: &dyn Keystore,
    install_id: &InstallId,
    ark: &Ark,
    current: &str,
    new: &str,
    params: Argon2Params,
    rng: &dyn Rng,
) -> Result<(), KeysError> {
    check_passphrase(new)?;
    let Some(stored) = keystore.get(&lock_entry_name(install_id))? else {
        return Err(KeysError::WrongPassphrase);
    };
    let blob = PassphraseWrapped::from_bytes(&stored).map_err(|_| malformed_locked_entry())?;
    let unwrapped = LockedArk::new(blob, *install_id).unlock_with_minimum(current, &params)?;
    // Compare check values, not the keys themselves (no variable-time compare of secrets).
    if kb_crypto::key_check_value(unwrapped.secret()) != kb_crypto::key_check_value(ark.secret()) {
        return Err(KeysError::Corrupted(
            "the locked root key is not the root key in use".to_owned(),
        ));
    }
    enable_lock(keystore, install_id, ark, new, params, rng)
}

fn malformed_locked_entry() -> KeysError {
    KeysError::Corrupted("the locked root key entry is malformed".to_owned())
}

#[cfg(test)]
mod tests;
