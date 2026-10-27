//! Root key rotation (`key-management.md`, "Revoking a device" step 2 and "Re-issuing
//! the recovery kit"). The pure part lives here; the core orchestrates re-wrapping,
//! the database rekey and the keystore update.

use kb_crypto::Secret32;
use kb_types::Rng;
use kb_types::devices::ROOT_ROTATION_CONTEXT;
use zeroize::Zeroizing;

use crate::codec::{Reader, put_bytes};
use crate::device::{DeviceKeys, DevicePublicKeys};
use crate::{Ark, KeysError};

/// A fresh random ARK (never derived from the old one).
#[must_use]
pub fn rotate_ark(rng: &dyn Rng) -> Ark {
    Ark::from_secret(Secret32::generate(rng))
}

/// The context string of a root key box of `generation`.
fn box_context(generation: u32) -> String {
    format!("{ROOT_ROTATION_CONTEXT}/{generation}")
}

/// What a `RootRotated` box carries to a remaining device: the new root key and the
/// recovery blob the rotating device issued with it (so every device can back up and
/// show the kit state).
#[derive(Clone)]
pub struct RotationBox {
    /// The new root key.
    pub ark: Ark,
    /// `HKDF(RcK, recovery-id)` of the new kit.
    pub recovery_id: [u8; 32],
    /// The new recovery blob (`recovery::RecoveryBlob::encode`).
    pub recovery_blob: Vec<u8>,
}

impl RotationBox {
    /// `ark [32] || recovery_id [32] || blob len u32 LE || blob`. The buffer is
    /// allocated once at its final size and wiped when dropped.
    #[must_use]
    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(
            (32 + 32 + 4usize).saturating_add(self.recovery_blob.len()),
        ));
        out.extend_from_slice(self.ark.secret().expose());
        out.extend_from_slice(&self.recovery_id);
        put_bytes(&mut out, &self.recovery_blob);
        out
    }

    /// Parses [`Self::encode`]; truncation or trailing bytes are
    /// [`KeysError::Corrupted`] (static text, never the bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let malformed = |error: crate::codec::DecodeError| {
            KeysError::Corrupted(format!("malformed rotation box: {}", error.0))
        };
        let mut r = Reader::new(bytes);
        let ark_bytes = Zeroizing::new(r.array::<32>().map_err(malformed)?);
        let recovery_id = r.array::<32>().map_err(malformed)?;
        let recovery_blob = r.bytes().map_err(malformed)?.to_vec();
        r.finish().map_err(malformed)?;
        Ok(Self {
            ark: Ark::from_secret(Secret32::from_bytes(*ark_bytes)),
            recovery_id,
            recovery_blob,
        })
    }
}

/// Seals a rotation box (of `generation`) to `recipient`'s agreement key for a
/// `RootRotated` log entry (`kb_crypto::seal_to`).
#[must_use]
pub fn seal_root_for(
    content: &RotationBox,
    generation: u32,
    recipient: &DevicePublicKeys,
    rng: &dyn Rng,
) -> Vec<u8> {
    kb_crypto::seal_to(
        &recipient.agreement,
        &box_context(generation),
        &content.encode(),
        rng,
    )
}

/// Opens this device's box of a `RootRotated` entry; a wrong recipient or a
/// tampered box is [`KeysError::Crypto`].
pub fn open_root_box(
    keys: &DeviceKeys,
    generation: u32,
    sealed: &[u8],
) -> Result<RotationBox, KeysError> {
    let plain = kb_crypto::open_sealed(keys.agreement(), &box_context(generation), sealed)?;
    RotationBox::decode(&plain)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{ark, device};
    use kb_crypto::CryptoError;
    use kb_types::testing::SeededRng;

    fn content() -> RotationBox {
        RotationBox {
            ark: ark(7),
            recovery_id: [0x1d; 32],
            recovery_blob: (0u8..93).collect(),
        }
    }

    fn assert_same(actual: &RotationBox, expected: &RotationBox) {
        assert_eq!(actual.ark, expected.ark);
        assert_eq!(actual.recovery_id, expected.recovery_id);
        assert_eq!(actual.recovery_blob, expected.recovery_blob);
    }

    /// The error of something that must not open (`RotationBox` has no `Debug`).
    fn refused(result: Result<RotationBox, KeysError>) -> KeysError {
        match result {
            Ok(_) => panic!("opened"),
            Err(err) => err,
        }
    }

    #[test]
    fn a_rotated_ark_is_a_fresh_draw_from_the_rng() {
        let rng = SeededRng::new(1);
        let first = rotate_ark(&rng);
        let second = rotate_ark(&rng);
        assert_ne!(first, second);
        assert_eq!(first.secret(), &Secret32::generate(&SeededRng::new(1)));
    }

    #[test]
    fn a_box_round_trips_to_its_recipient() {
        let (_, b) = device(2);
        let rng = SeededRng::new(9);
        for generation in [1, 2, u32::MAX] {
            let sealed = seal_root_for(&content(), generation, &b.public(), &rng);
            assert_same(&open_root_box(&b, generation, &sealed).unwrap(), &content());
        }
        let empty = RotationBox {
            recovery_blob: Vec::new(),
            ..content()
        };
        let sealed = seal_root_for(&empty, 1, &b.public(), &rng);
        assert_same(&open_root_box(&b, 1, &sealed).unwrap(), &empty);

        let again = seal_root_for(&content(), 1, &b.public(), &rng);
        assert_ne!(again, seal_root_for(&content(), 1, &b.public(), &rng));
    }

    #[test]
    fn a_box_has_the_documented_layout() {
        let c = content();
        let mut expected = c.ark.secret().expose().to_vec();
        expected.extend_from_slice(&[0x1d; 32]);
        expected.extend_from_slice(&93u32.to_le_bytes());
        expected.extend(0u8..93);
        let encoded = c.encode();
        assert_eq!(encoded.as_slice(), expected.as_slice());
        assert_eq!(encoded.capacity(), encoded.len(), "allocated once");

        // A sealed box (`kb_crypto::seal_to`) under `<ROOT_ROTATION_CONTEXT>/<generation>`.
        let (_, b) = device(2);
        let sealed = seal_root_for(&c, 3, &b.public(), &SeededRng::new(9));
        assert_eq!(
            sealed.len(),
            expected.len() + kb_crypto::agreement::SEALED_OVERHEAD
        );
        let plain =
            kb_crypto::open_sealed(b.agreement(), "atomik-kb/v1/root-rotation/3", &sealed).unwrap();
        assert_eq!(plain.as_slice(), expected.as_slice());
    }

    #[test]
    fn another_device_cannot_open_a_box() {
        let (_, b) = device(2);
        let (_, c) = device(3);
        let sealed = seal_root_for(&content(), 1, &b.public(), &SeededRng::new(9));
        let err = refused(open_root_box(&c, 1, &sealed));
        assert!(
            matches!(err, KeysError::Crypto(CryptoError::Authentication)),
            "{err:?}"
        );
    }

    #[test]
    fn a_box_opens_only_for_its_generation() {
        let (_, b) = device(2);
        let sealed = seal_root_for(&content(), 2, &b.public(), &SeededRng::new(9));
        for generation in [0, 1, 3, 20, u32::MAX] {
            let err = refused(open_root_box(&b, generation, &sealed));
            assert!(
                matches!(err, KeysError::Crypto(CryptoError::Authentication)),
                "generation {generation}: {err:?}"
            );
        }
        assert!(open_root_box(&b, 2, &sealed).is_ok());
    }

    #[test]
    fn a_tampered_or_truncated_box_is_refused() {
        let (_, b) = device(2);
        let sealed = seal_root_for(&content(), 1, &b.public(), &SeededRng::new(9));
        for i in 0..sealed.len() {
            let mut tampered = sealed.clone();
            tampered[i] ^= 0x01;
            let err = refused(open_root_box(&b, 1, &tampered));
            assert!(matches!(err, KeysError::Crypto(_)), "byte {i}: {err:?}");
        }
        for len in 0..sealed.len() {
            let err = refused(open_root_box(&b, 1, &sealed[..len]));
            assert!(matches!(err, KeysError::Crypto(_)), "{len} bytes: {err:?}");
        }
        let mut longer = sealed.clone();
        longer.push(0);
        assert!(matches!(
            refused(open_root_box(&b, 1, &longer)),
            KeysError::Crypto(_)
        ));
    }

    #[test]
    fn a_box_whose_content_is_malformed_is_corrupted() {
        let (_, b) = device(2);
        let encoded = content().encode();
        let rng = SeededRng::new(9);
        let seal =
            |plain: &[u8]| kb_crypto::seal_to(&b.public().agreement, &box_context(1), plain, &rng);
        for plain in [
            &encoded[..67],
            &encoded[..encoded.len() - 1],
            &[encoded.as_slice(), &[0]].concat()[..],
            &[0u8; 32][..],
        ] {
            let err = refused(open_root_box(&b, 1, &seal(plain)));
            assert!(
                matches!(err, KeysError::Corrupted(_)),
                "{} bytes: {err:?}",
                plain.len()
            );
            let message = err.to_string();
            assert!(!message.contains("1d1d"), "{message}");
        }
        assert!(open_root_box(&b, 1, &seal(&encoded)).is_ok());
    }

    #[test]
    fn decoding_is_strict() {
        let encoded = content().encode();
        assert_same(&RotationBox::decode(&encoded).unwrap(), &content());
        for len in 0..encoded.len() {
            assert!(
                matches!(
                    RotationBox::decode(&encoded[..len]),
                    Err(KeysError::Corrupted(_))
                ),
                "{len} bytes"
            );
        }
        let mut wrong_len = encoded.to_vec();
        wrong_len[64..68].copy_from_slice(&94u32.to_le_bytes());
        assert!(matches!(
            RotationBox::decode(&wrong_len),
            Err(KeysError::Corrupted(_))
        ));
        wrong_len[64..68].copy_from_slice(&92u32.to_le_bytes());
        assert!(matches!(
            RotationBox::decode(&wrong_len),
            Err(KeysError::Corrupted(_))
        ));
    }
}
