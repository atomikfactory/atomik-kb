//! Argon2id passphrase wrapping for the optional app lock (ADR-0018,
//! `encryption-design.md`). The only place a user-chosen secret touches a key.
//!
//! Blob layout ([`PassphraseWrapped::to_bytes`], 101 bytes):
//!
//! ```text
//! u8 version = 1 | u32 LE memory_kib | u32 LE iterations | u32 LE parallelism |
//! [16] salt | [72] wrapped key (nonce || ciphertext || tag)
//! ```

use argon2::{Algorithm, Argon2, AssociatedData, Block, ParamsBuilder, Version};
use kb_types::Rng;
use zeroize::Zeroizing;

use crate::codec::Reader;
use crate::wrap::{WRAPPED_KEY_LEN, WrappedKey, unwrap_key, wrap_key};
use crate::{CryptoError, Secret32};

/// Argon2id parameters stored with the wrapped blob.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Argon2Params {
    /// Memory in KiB.
    pub memory_kib: u32,
    /// Passes.
    pub iterations: u32,
    /// Lanes.
    pub parallelism: u32,
}

impl Argon2Params {
    /// RFC 9106 first recommendation: 64 MiB, t = 3, p = 4.
    pub const DEFAULT: Self = Self {
        memory_kib: 65_536,
        iterations: 3,
        parallelism: 4,
    };

    /// What production readers accept at least (equal to the default).
    pub const MINIMUM: Self = Self::DEFAULT;

    /// `true` when every parameter is at least the corresponding minimum.
    #[must_use]
    pub const fn at_least(&self, minimum: &Self) -> bool {
        self.memory_kib >= minimum.memory_kib
            && self.iterations >= minimum.iterations
            && self.parallelism >= minimum.parallelism
    }

    /// Structural validity (independent of any minimum): at least one lane and one
    /// pass, at most 2^24 - 1 lanes, at least 8 KiB of memory per lane.
    fn check(&self) -> Result<(), CryptoError> {
        if self.parallelism == 0 || self.parallelism > argon2::Params::MAX_P_COST {
            return Err(CryptoError::Encoding(format!(
                "Argon2id parallelism must be between 1 and {}, got {}",
                argon2::Params::MAX_P_COST,
                self.parallelism
            )));
        }
        if self.iterations == 0 {
            return Err(CryptoError::Encoding(
                "Argon2id needs at least one iteration".to_owned(),
            ));
        }
        if u64::from(self.memory_kib) < 8 * u64::from(self.parallelism) {
            return Err(CryptoError::Encoding(format!(
                "Argon2id needs at least 8 KiB of memory per lane, got {} KiB for {} lanes",
                self.memory_kib, self.parallelism
            )));
        }
        Ok(())
    }
}

/// Salt length.
pub const SALT_LEN: usize = 16;
/// Layout version of [`PassphraseWrapped::to_bytes`].
pub const PASSPHRASE_BLOB_VERSION: u8 = 1;
/// Encoded size of a [`PassphraseWrapped`]: version, three `u32`, salt, wrapped key.
pub const PASSPHRASE_WRAPPED_LEN: usize = 1 + 3 * 4 + SALT_LEN + WRAPPED_KEY_LEN;

/// Length of the Argon2id output, the key-encryption key.
const KEK_LEN: usize = 32;

/// A key wrapped under a passphrase: the parameters and salt travel with the blob.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PassphraseWrapped {
    /// Parameters used.
    pub params: Argon2Params,
    /// Random salt.
    pub salt: [u8; SALT_LEN],
    /// The key wrapped under the derived key.
    pub wrapped: WrappedKey,
}

impl PassphraseWrapped {
    /// Fixed layout: version `1`, three `u32` LE parameters, salt, wrapped key
    /// ([`PASSPHRASE_WRAPPED_LEN`] bytes).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PASSPHRASE_WRAPPED_LEN);
        out.push(PASSPHRASE_BLOB_VERSION);
        out.extend_from_slice(&self.params.memory_kib.to_le_bytes());
        out.extend_from_slice(&self.params.iterations.to_le_bytes());
        out.extend_from_slice(&self.params.parallelism.to_le_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.wrapped.to_bytes());
        out
    }

    /// Parses [`Self::to_bytes`] output; anything else is [`CryptoError::Encoding`]:
    /// a wrong length, an unknown version, or parameters Argon2id cannot run with
    /// (no lane, no pass, more than 2^24 - 1 lanes, less than 8 KiB per lane).
    /// Weak but valid parameters decode; [`unwrap_with_passphrase`] refuses them.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != PASSPHRASE_WRAPPED_LEN {
            return Err(CryptoError::Encoding(format!(
                "a passphrase-wrapped key is {PASSPHRASE_WRAPPED_LEN} bytes, got {}",
                bytes.len()
            )));
        }
        let mut reader = Reader::new(bytes);
        let version = reader.u8("passphrase blob version")?;
        if version != PASSPHRASE_BLOB_VERSION {
            return Err(CryptoError::Encoding(format!(
                "unknown passphrase blob version {version}"
            )));
        }
        let params = Argon2Params {
            memory_kib: reader.u32_le("Argon2id memory")?,
            iterations: reader.u32_le("Argon2id iterations")?,
            parallelism: reader.u32_le("Argon2id parallelism")?,
        };
        params.check()?;
        let salt = reader.array::<SALT_LEN>("salt")?;
        let wrapped = WrappedKey::from_bytes(reader.rest())?;
        Ok(Self {
            params,
            salt,
            wrapped,
        })
    }
}

/// Argon2id (RFC 9106, version 0x13) of `passphrase` into a 32-byte key. Production
/// passes an empty `secret` and `associated_data`; the RFC known-answer test uses
/// both. The memory blocks (derived from the passphrase) are wiped afterwards.
pub(crate) fn argon2id(
    passphrase: &[u8],
    salt: &[u8],
    params: &Argon2Params,
    secret: &[u8],
    associated_data: &[u8],
) -> Result<Zeroizing<[u8; KEK_LEN]>, CryptoError> {
    params.check()?;
    let argon2_params = ParamsBuilder::new()
        .m_cost(params.memory_kib)
        .t_cost(params.iterations)
        .p_cost(params.parallelism)
        .output_len(KEK_LEN)
        .data(AssociatedData::new(associated_data).map_err(argon2_error)?)
        .build()
        .map_err(argon2_error)?;
    let block_count = argon2_params.block_count();
    let context =
        Argon2::new_with_secret(secret, Algorithm::Argon2id, Version::V0x13, argon2_params)
            .map_err(argon2_error)?;

    // Our own allocation (instead of the crate's) so the blocks can be wiped; a
    // failed allocation is an error, not an abort.
    let mut blocks: Zeroizing<Vec<Block>> = Zeroizing::new(Vec::new());
    blocks.try_reserve_exact(block_count).map_err(|_| {
        CryptoError::Encoding(format!(
            "cannot allocate {} KiB for Argon2id",
            params.memory_kib
        ))
    })?;
    blocks.resize(block_count, Block::new());

    let mut out = Zeroizing::new([0u8; KEK_LEN]);
    context
        .hash_password_into_with_memory(passphrase, salt, out.as_mut_slice(), blocks.as_mut_slice())
        .map_err(argon2_error)?;
    Ok(out)
}

/// The Argon2 error names the parameter, never an input.
fn argon2_error(error: argon2::Error) -> CryptoError {
    CryptoError::Encoding(format!("Argon2id: {error}"))
}

/// The key-encryption key for a blob: Argon2id(passphrase, salt, params).
fn passphrase_kek(
    passphrase: &[u8],
    salt: &[u8; SALT_LEN],
    params: &Argon2Params,
) -> Result<Secret32, CryptoError> {
    let okm = argon2id(passphrase, salt, params, &[], &[])?;
    Ok(Secret32::from_bytes(*okm))
}

/// Derives a 32-byte key with Argon2id(`passphrase`, `salt`, `params`) and wraps
/// `key` under it with [`crate::wrap_key`] (AAD subject = `subject`).
///
/// The salt is drawn from `rng`. `params` must be structurally valid (see
/// [`PassphraseWrapped::from_bytes`]) or the result is [`CryptoError::Encoding`];
/// they are not compared with [`Argon2Params::MINIMUM`] here (tests wrap with small
/// parameters), only readers enforce a minimum.
pub fn wrap_with_passphrase(
    key: &Secret32,
    passphrase: &[u8],
    params: Argon2Params,
    subject: &[u8],
    rng: &dyn Rng,
) -> Result<PassphraseWrapped, CryptoError> {
    params.check()?;
    let mut salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut salt);
    let kek = passphrase_kek(passphrase, &salt, &params)?;
    Ok(PassphraseWrapped {
        params,
        salt,
        wrapped: wrap_key(key, &kek, subject, rng),
    })
}

/// Reverses [`wrap_with_passphrase`]. Parameters below `minimum` are
/// [`CryptoError::WeakParameters`] (checked before any work); a wrong passphrase is
/// [`CryptoError::Authentication`] after the full Argon2id cost.
pub fn unwrap_with_passphrase(
    blob: &PassphraseWrapped,
    passphrase: &[u8],
    subject: &[u8],
    minimum: &Argon2Params,
) -> Result<Secret32, CryptoError> {
    if !blob.params.at_least(minimum) {
        return Err(CryptoError::WeakParameters);
    }
    let kek = passphrase_kek(passphrase, &blob.salt, &blob.params)?;
    unwrap_key(&blob.wrapped, &kek, subject)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{hex, kat, unhex};
    use kb_types::testing::SeededRng;

    /// Small parameters so the unit tests stay fast (8 MiB is the golden's size).
    const SMALL: Argon2Params = Argon2Params {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };

    fn key() -> Secret32 {
        Secret32::from_bytes([0x42; 32])
    }

    #[test]
    fn matches_rfc9106_section_5_3() {
        let file = kat("kat-argon2id.json");
        let vectors = file["vectors"].as_array().unwrap();
        assert_eq!(vectors.len(), 1);
        for vector in vectors {
            assert_eq!(vector["version"], 19, "Argon2 version 0x13");
            assert_eq!(vector["tag_length"], 32);
            let params = Argon2Params {
                memory_kib: u32::try_from(vector["memory_kib"].as_u64().unwrap()).unwrap(),
                iterations: u32::try_from(vector["iterations"].as_u64().unwrap()).unwrap(),
                parallelism: u32::try_from(vector["parallelism"].as_u64().unwrap()).unwrap(),
            };
            let tag = argon2id(
                &unhex(vector["password_hex"].as_str().unwrap()),
                &unhex(vector["salt_hex"].as_str().unwrap()),
                &params,
                &unhex(vector["secret_hex"].as_str().unwrap()),
                &unhex(vector["associated_data_hex"].as_str().unwrap()),
            )
            .unwrap();
            assert_eq!(hex(tag.as_slice()), vector["tag_hex"].as_str().unwrap());
        }
    }

    #[test]
    fn the_production_derivation_is_plain_argon2id() {
        // Empty secret and associated data are the same as none at all.
        let ours = argon2id(b"passphrase", &[7; 16], &SMALL, &[], &[]).unwrap();
        let params = argon2::Params::new(64, 1, 1, Some(32)).unwrap();
        let mut expected = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(b"passphrase", &[7; 16], &mut expected)
            .unwrap();
        assert_eq!(*ours, expected);
    }

    #[test]
    fn parameters_are_pinned() {
        assert_eq!(
            Argon2Params::DEFAULT,
            Argon2Params {
                memory_kib: 65_536,
                iterations: 3,
                parallelism: 4
            }
        );
        assert_eq!(Argon2Params::MINIMUM, Argon2Params::DEFAULT);
        let min = Argon2Params::MINIMUM;
        assert!(min.at_least(&min));
        for weaker in [
            Argon2Params {
                memory_kib: 65_535,
                ..min
            },
            Argon2Params {
                iterations: 2,
                ..min
            },
            Argon2Params {
                parallelism: 3,
                ..min
            },
        ] {
            assert!(!weaker.at_least(&min), "{weaker:?}");
        }
        assert!(
            Argon2Params {
                memory_kib: 1 << 20,
                iterations: 4,
                parallelism: 8
            }
            .at_least(&min)
        );
    }

    #[test]
    fn round_trip() {
        let rng = SeededRng::new(1);
        let blob =
            wrap_with_passphrase(&key(), b"correct horse", SMALL, b"ark:install", &rng).unwrap();
        assert_eq!(blob.params, SMALL);
        let bytes = blob.to_bytes();
        assert_eq!(bytes.len(), PASSPHRASE_WRAPPED_LEN);
        assert_eq!(PASSPHRASE_WRAPPED_LEN, 101);
        let decoded = PassphraseWrapped::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, blob);
        assert_eq!(
            unwrap_with_passphrase(&decoded, b"correct horse", b"ark:install", &SMALL).unwrap(),
            key()
        );
    }

    #[test]
    fn layout_is_version_params_salt_wrapped() {
        // Encoding needs no derivation: large but valid parameters, distinct bytes.
        let mut wrapped_bytes = [0u8; WRAPPED_KEY_LEN];
        for (i, b) in wrapped_bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap();
        }
        let blob = PassphraseWrapped {
            params: Argon2Params {
                memory_kib: 0x0403_0201,
                iterations: 0x0807_0605,
                parallelism: 0x000b_0a09,
            },
            salt: [0xee; SALT_LEN],
            wrapped: WrappedKey::from_bytes(&wrapped_bytes).unwrap(),
        };
        let bytes = blob.to_bytes();
        assert_eq!(bytes.len(), PASSPHRASE_WRAPPED_LEN);
        assert_eq!(bytes[0], PASSPHRASE_BLOB_VERSION);
        assert_eq!(PASSPHRASE_BLOB_VERSION, 1);
        assert_eq!(&bytes[1..13], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 0x0a, 0x0b, 0]);
        assert_eq!(&bytes[13..29], &[0xee; SALT_LEN]);
        assert_eq!(&bytes[29..], &wrapped_bytes);
        assert_eq!(PassphraseWrapped::from_bytes(&bytes).unwrap(), blob);
    }

    #[test]
    fn wrong_passphrase_or_subject_is_authentication() {
        let rng = SeededRng::new(3);
        let blob = wrap_with_passphrase(&key(), b"right", SMALL, b"subject", &rng).unwrap();
        assert_eq!(
            unwrap_with_passphrase(&blob, b"wrong", b"subject", &SMALL),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            unwrap_with_passphrase(&blob, b"right", b"other", &SMALL),
            Err(CryptoError::Authentication)
        );
        assert_eq!(
            unwrap_with_passphrase(&blob, b"", b"subject", &SMALL),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn tampered_parameters_or_salt_fail_authentication() {
        let rng = SeededRng::new(4);
        let blob = wrap_with_passphrase(&key(), b"pw", SMALL, b"s", &rng).unwrap();
        let mut more_memory = blob.clone();
        more_memory.params.memory_kib += 8;
        let mut more_passes = blob.clone();
        more_passes.params.iterations += 1;
        let mut other_salt = blob.clone();
        other_salt.salt[0] ^= 1;
        for tampered in [more_memory, more_passes, other_salt] {
            assert_eq!(
                unwrap_with_passphrase(&tampered, b"pw", b"s", &SMALL),
                Err(CryptoError::Authentication)
            );
        }
    }

    #[test]
    fn weaker_parameters_are_refused_before_any_work() {
        // 4 TiB would never finish (or allocate); the refusal must come first.
        let absurd = PassphraseWrapped {
            params: Argon2Params {
                memory_kib: u32::MAX,
                iterations: 1,
                parallelism: 1,
            },
            salt: [0; SALT_LEN],
            wrapped: WrappedKey::from_bytes(&[0; WRAPPED_KEY_LEN]).unwrap(),
        };
        let minimum = Argon2Params {
            memory_kib: 8,
            iterations: 2,
            parallelism: 1,
        };
        assert_eq!(
            unwrap_with_passphrase(&absurd, b"pw", b"s", &minimum),
            Err(CryptoError::WeakParameters)
        );
        let rng = SeededRng::new(5);
        let small = wrap_with_passphrase(&key(), b"pw", SMALL, b"s", &rng).unwrap();
        assert_eq!(
            unwrap_with_passphrase(&small, b"pw", b"s", &Argon2Params::MINIMUM),
            Err(CryptoError::WeakParameters)
        );
        // Weak and invalid together: still WeakParameters (the minimum is checked first).
        let mut no_lanes = small.clone();
        no_lanes.params.parallelism = 0;
        assert_eq!(
            unwrap_with_passphrase(&no_lanes, b"pw", b"s", &Argon2Params::MINIMUM),
            Err(CryptoError::WeakParameters)
        );
    }

    #[test]
    fn invalid_parameters_are_encoding_errors() {
        let rng = SeededRng::new(6);
        for params in [
            Argon2Params {
                memory_kib: 64,
                iterations: 1,
                parallelism: 0,
            },
            Argon2Params {
                memory_kib: 15,
                iterations: 1,
                parallelism: 2,
            },
            Argon2Params {
                memory_kib: 64,
                iterations: 0,
                parallelism: 1,
            },
            Argon2Params {
                memory_kib: u32::MAX,
                iterations: 1,
                parallelism: 1 << 24,
            },
        ] {
            assert!(
                matches!(
                    wrap_with_passphrase(&key(), b"pw", params, b"s", &rng),
                    Err(CryptoError::Encoding(_))
                ),
                "{params:?}"
            );
            let mut blob = wrap_with_passphrase(&key(), b"pw", SMALL, b"s", &rng)
                .unwrap()
                .to_bytes();
            blob[1..5].copy_from_slice(&params.memory_kib.to_le_bytes());
            blob[5..9].copy_from_slice(&params.iterations.to_le_bytes());
            blob[9..13].copy_from_slice(&params.parallelism.to_le_bytes());
            assert!(
                matches!(
                    PassphraseWrapped::from_bytes(&blob),
                    Err(CryptoError::Encoding(_))
                ),
                "{params:?}"
            );
        }
        // Exactly 8 KiB per lane is the smallest valid memory.
        let edge = Argon2Params {
            memory_kib: 16,
            iterations: 1,
            parallelism: 2,
        };
        assert!(wrap_with_passphrase(&key(), b"pw", edge, b"s", &rng).is_ok());
    }

    #[test]
    fn from_bytes_refuses_bad_lengths_and_versions() {
        let rng = SeededRng::new(7);
        let bytes = wrap_with_passphrase(&key(), b"pw", SMALL, b"s", &rng)
            .unwrap()
            .to_bytes();
        for len in [0, 1, 100, 102] {
            let mut resized = bytes.clone();
            resized.resize(len, 0);
            assert!(matches!(
                PassphraseWrapped::from_bytes(&resized),
                Err(CryptoError::Encoding(_))
            ));
        }
        for version in [0u8, 2, 255] {
            let mut other = bytes.clone();
            other[0] = version;
            assert_eq!(
                PassphraseWrapped::from_bytes(&other),
                Err(CryptoError::Encoding(format!(
                    "unknown passphrase blob version {version}"
                )))
            );
        }
    }

    #[test]
    fn each_wrap_draws_a_fresh_salt() {
        let rng = SeededRng::new(8);
        let a = wrap_with_passphrase(&key(), b"pw", SMALL, b"s", &rng).unwrap();
        let b = wrap_with_passphrase(&key(), b"pw", SMALL, b"s", &rng).unwrap();
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.wrapped, b.wrapped);
    }
}
