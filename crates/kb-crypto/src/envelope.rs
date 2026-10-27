//! Envelope v1: the one layout for every persisted or transmitted ciphertext
//! (`encryption-design.md`, "Envelopes").
//!
//! ```text
//! u8 version = 1 | u8 suite = 1 | u8 kind | u32 LE header_len | header |
//! [24] nonce | u32 LE ciphertext_len | ciphertext (with tag) | [64] signature (optional)
//! ```
//!
//! AAD = `version || suite || kind || header`; the signature (when present) is over
//! everything before it with context `envelope/<kind>`. A signature is present when
//! exactly [`SIGNATURE_LEN`] bytes follow the ciphertext; any other number of
//! trailing bytes is malformed.

use kb_types::Rng;
use zeroize::Zeroizing;

use crate::aead::{self, NONCE_LEN, Nonce24, TAG_LEN};
use crate::codec::Reader;
use crate::sign::{SIGNATURE_LEN, Signature, SigningKey, VerifyingKey, sign, verify};
use crate::{CryptoError, Secret32};

/// Envelope layout version.
pub const ENVELOPE_VERSION: u8 = 1;
/// Cryptographic suite id (ADR-0013).
pub const SUITE_ID: u8 = 1;

/// Bytes of an envelope around its header and ciphertext, without a signature:
/// version, suite, kind, two `u32` lengths and the nonce.
pub const ENVELOPE_OVERHEAD: usize = 3 + 4 + NONCE_LEN + 4;

/// What an envelope carries. The byte values are on disk and on the wire: never renumber.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum EnvelopeKind {
    /// A sync entry (M6).
    Entry = 1,
    /// A remote snapshot (M6).
    Snapshot = 2,
    /// A keyring record or tombstone in transit.
    Keyring = 3,
    /// An attachment chunk (M5).
    Chunk = 4,
    /// A backup file section.
    Backup = 5,
    /// A pairing message (M7).
    Pairing = 6,
    /// The account document in transit (M6).
    AccountDoc = 7,
    /// A signed head record (M6).
    Head = 8,
    /// A membership log entry of a shared space (M13, ADR-0043): signed by a device, its private
    /// fields sealed under the membership-meta key.
    Membership = 9,
    /// A presence frame (M13 slice 3, ADR-0050): signed by a device, sealed under an SK-derived key.
    Presence = 10,
    /// A view-link publication (M13 slice 4, ADR-0051): unsigned, sealed under the link key.
    Publication = 11,
}

impl EnvelopeKind {
    /// Every kind, in byte order.
    pub const ALL: [Self; 11] = [
        Self::Entry,
        Self::Snapshot,
        Self::Keyring,
        Self::Chunk,
        Self::Backup,
        Self::Pairing,
        Self::AccountDoc,
        Self::Head,
        Self::Membership,
        Self::Presence,
        Self::Publication,
    ];

    /// The byte in the envelope.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Parses the byte; unknown values are `None`.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Entry),
            2 => Some(Self::Snapshot),
            3 => Some(Self::Keyring),
            4 => Some(Self::Chunk),
            5 => Some(Self::Backup),
            6 => Some(Self::Pairing),
            7 => Some(Self::AccountDoc),
            8 => Some(Self::Head),
            9 => Some(Self::Membership),
            10 => Some(Self::Presence),
            11 => Some(Self::Publication),
            _ => None,
        }
    }

    /// Signature context, `envelope/<kind>`.
    #[must_use]
    pub const fn context(self) -> &'static str {
        match self {
            Self::Entry => "envelope/entry",
            Self::Snapshot => "envelope/snapshot",
            Self::Keyring => "envelope/keyring",
            Self::Chunk => "envelope/chunk",
            Self::Backup => "envelope/backup",
            Self::Pairing => "envelope/pairing",
            Self::AccountDoc => "envelope/account-doc",
            Self::Head => "envelope/head",
            Self::Membership => "envelope/membership",
            Self::Presence => "envelope/presence",
            Self::Publication => "envelope/publication",
        }
    }
}

/// A decoded envelope (ciphertext still sealed).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Envelope {
    /// Kind.
    pub kind: EnvelopeKind,
    /// Kind-specific plaintext header, authenticated as AAD.
    pub header: Vec<u8>,
    /// Nonce.
    pub nonce: Nonce24,
    /// Ciphertext including the tag.
    pub ciphertext: Vec<u8>,
    /// Signature over everything before it, when the kind is signed.
    pub signature: Option<Signature>,
}

impl Envelope {
    /// Serialises the envelope.
    ///
    /// A header or ciphertext longer than `u32::MAX` bytes cannot be represented; the
    /// result is then empty, which [`Self::decode`] refuses (closed failure).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        encode_parts(
            self.kind,
            &self.header,
            &self.nonce,
            &self.ciphertext,
            self.signature.as_ref(),
        )
    }

    /// Parses an envelope; unknown versions, suites and kinds, truncation and
    /// trailing bytes are [`CryptoError::Encoding`]. Nothing is verified.
    ///
    /// The encoding is canonical: `decode(bytes)?.encode() == bytes`.
    pub fn decode(bytes: &[u8]) -> Result<Self, CryptoError> {
        let mut reader = Reader::new(bytes);
        let version = reader.u8("envelope version")?;
        if version != ENVELOPE_VERSION {
            return Err(CryptoError::Encoding(format!(
                "unknown envelope version {version}"
            )));
        }
        let suite = reader.u8("suite id")?;
        if suite != SUITE_ID {
            return Err(CryptoError::Encoding(format!("unknown suite {suite}")));
        }
        let kind_byte = reader.u8("envelope kind")?;
        let kind = EnvelopeKind::from_byte(kind_byte)
            .ok_or_else(|| CryptoError::Encoding(format!("unknown envelope kind {kind_byte}")))?;
        let header = reader.len_prefixed("envelope header")?.to_vec();
        let nonce = Nonce24::from_bytes(reader.array::<NONCE_LEN>("nonce")?);
        let ciphertext = reader.len_prefixed("ciphertext")?;
        if ciphertext.len() < TAG_LEN {
            return Err(CryptoError::Encoding(
                "ciphertext shorter than its tag".to_owned(),
            ));
        }
        let signature = match reader.rest().len() {
            0 => None,
            SIGNATURE_LEN => Some(Signature::from_bytes(reader.rest())?),
            extra => {
                return Err(CryptoError::Encoding(format!(
                    "{extra} unexpected bytes after the ciphertext"
                )));
            }
        };
        Ok(Self {
            kind,
            header,
            nonce,
            ciphertext: ciphertext.to_vec(),
            signature,
        })
    }

    /// `version || suite || kind || header`.
    #[must_use]
    pub fn aad(&self) -> Vec<u8> {
        aad_for(self.kind, &self.header)
    }
}

/// `version || suite || kind || header`: the AAD of every envelope. The prefix has a
/// fixed length, so the header is recovered unambiguously.
fn aad_for(kind: EnvelopeKind, header: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(3 + header.len());
    aad.extend_from_slice(&[ENVELOPE_VERSION, SUITE_ID, kind.to_byte()]);
    aad.extend_from_slice(header);
    aad
}

fn encode_parts(
    kind: EnvelopeKind,
    header: &[u8],
    nonce: &Nonce24,
    ciphertext: &[u8],
    signature: Option<&Signature>,
) -> Vec<u8> {
    let (Ok(header_len), Ok(ciphertext_len)) =
        (u32::try_from(header.len()), u32::try_from(ciphertext.len()))
    else {
        return Vec::new();
    };
    let mut out =
        Vec::with_capacity(ENVELOPE_OVERHEAD + header.len() + ciphertext.len() + SIGNATURE_LEN);
    out.extend_from_slice(&[ENVELOPE_VERSION, SUITE_ID, kind.to_byte()]);
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(nonce.as_bytes());
    out.extend_from_slice(&ciphertext_len.to_le_bytes());
    out.extend_from_slice(ciphertext);
    if let Some(signature) = signature {
        out.extend_from_slice(&signature.to_bytes());
    }
    out
}

/// Encrypts `plaintext` into an envelope of `kind` with `header`, signing it when a
/// `signer` is given.
///
/// The nonce is drawn from `rng`; the signature (context [`EnvelopeKind::context`])
/// covers every byte before it. Headers or ciphertexts beyond `u32::MAX` bytes give
/// an empty result (see [`Envelope::encode`]).
#[must_use]
pub fn seal_envelope(
    kind: EnvelopeKind,
    header: &[u8],
    key: &Secret32,
    plaintext: &[u8],
    signer: Option<&SigningKey>,
    rng: &dyn Rng,
) -> Vec<u8> {
    let nonce = Nonce24::random(rng);
    let ciphertext = aead::seal(key, &nonce, &aad_for(kind, header), plaintext);
    let mut out = encode_parts(kind, header, &nonce, &ciphertext, None);
    if out.is_empty() {
        return out;
    }
    if let Some(signer) = signer {
        let signature = sign(signer, kind.context(), &out);
        out.extend_from_slice(&signature.to_bytes());
    }
    out
}

/// Decodes, verifies (a `verifier` makes the signature mandatory) and decrypts an
/// envelope, returning it (for its header) and the plaintext.
///
/// Malformed bytes are [`CryptoError::Encoding`]. With a `verifier`, a missing or
/// invalid signature is [`CryptoError::Authentication`] and is checked before
/// anything is decrypted; without one, a present signature is not checked. A wrong
/// key or tampered header, nonce or ciphertext is [`CryptoError::Authentication`].
pub fn open_envelope(
    bytes: &[u8],
    key: &Secret32,
    verifier: Option<&VerifyingKey>,
) -> Result<(Envelope, Zeroizing<Vec<u8>>), CryptoError> {
    let envelope = Envelope::decode(bytes)?;
    if let Some(verifier) = verifier {
        let signature = envelope
            .signature
            .as_ref()
            .ok_or(CryptoError::Authentication)?;
        // Decoding is canonical, so the signed bytes are the input minus the signature.
        let signed_len = bytes
            .len()
            .checked_sub(SIGNATURE_LEN)
            .ok_or(CryptoError::Authentication)?;
        let signed = bytes.get(..signed_len).ok_or(CryptoError::Authentication)?;
        verify(verifier, envelope.kind.context(), signed, signature)?;
    }
    let plaintext = aead::open(key, &envelope.nonce, &envelope.aad(), &envelope.ciphertext)?;
    Ok((envelope, plaintext))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use kb_types::testing::SeededRng;

    fn key() -> Secret32 {
        Secret32::from_bytes([0x11; 32])
    }

    fn signer() -> SigningKey {
        SigningKey::from_seed(&Secret32::from_bytes([0x22; 32]))
    }

    /// Offsets of the regions of an envelope with a 5-byte header.
    struct Regions {
        header: core::ops::Range<usize>,
        nonce: core::ops::Range<usize>,
        ciphertext_len: core::ops::Range<usize>,
        ciphertext: core::ops::Range<usize>,
        signature: core::ops::Range<usize>,
    }

    fn regions(header_len: usize, ciphertext_len: usize) -> Regions {
        let header = 7..7 + header_len;
        let nonce = header.end..header.end + NONCE_LEN;
        let ciphertext_len_range = nonce.end..nonce.end + 4;
        let ciphertext = ciphertext_len_range.end..ciphertext_len_range.end + ciphertext_len;
        let signature = ciphertext.end..ciphertext.end + SIGNATURE_LEN;
        Regions {
            header,
            nonce,
            ciphertext_len: ciphertext_len_range,
            ciphertext,
            signature,
        }
    }

    #[test]
    fn kinds_and_contexts_are_frozen() {
        let expected = [
            (1, "envelope/entry"),
            (2, "envelope/snapshot"),
            (3, "envelope/keyring"),
            (4, "envelope/chunk"),
            (5, "envelope/backup"),
            (6, "envelope/pairing"),
            (7, "envelope/account-doc"),
            (8, "envelope/head"),
            (9, "envelope/membership"),
            (10, "envelope/presence"),
            (11, "envelope/publication"),
        ];
        for (kind, (byte, context)) in EnvelopeKind::ALL.into_iter().zip(expected) {
            assert_eq!(kind.to_byte(), byte);
            assert_eq!(EnvelopeKind::from_byte(byte), Some(kind));
            assert_eq!(kind.context(), context);
        }
        for byte in [0u8, 12, 0x80, 0xff] {
            assert_eq!(EnvelopeKind::from_byte(byte), None);
        }
        assert_eq!((ENVELOPE_VERSION, SUITE_ID), (1, 1));
        assert_eq!(ENVELOPE_OVERHEAD, 35);
    }

    #[test]
    fn round_trip_for_every_kind_signed_and_unsigned() {
        let rng = SeededRng::new(1);
        for kind in EnvelopeKind::ALL {
            for signed in [false, true] {
                let signing = signer();
                let header = [kind.to_byte(), 0xaa, 0xbb];
                let plaintext = format!("plaintext of {kind:?}");
                let bytes = seal_envelope(
                    kind,
                    &header,
                    &key(),
                    plaintext.as_bytes(),
                    signed.then_some(&signing),
                    &rng,
                );
                let expected_len = ENVELOPE_OVERHEAD
                    + header.len()
                    + plaintext.len()
                    + TAG_LEN
                    + if signed { SIGNATURE_LEN } else { 0 };
                assert_eq!(bytes.len(), expected_len);

                let decoded = Envelope::decode(&bytes).unwrap();
                assert_eq!(decoded.kind, kind);
                assert_eq!(decoded.header, header);
                assert_eq!(decoded.signature.is_some(), signed);
                assert_eq!(decoded.encode(), bytes, "canonical encoding");

                let verifier = signing.verifying_key();
                let (opened, text) =
                    open_envelope(&bytes, &key(), signed.then_some(&verifier)).unwrap();
                assert_eq!(opened, decoded);
                assert_eq!(text.as_slice(), plaintext.as_bytes());
                // Without a verifier a signature is simply not checked.
                assert!(open_envelope(&bytes, &key(), None).is_ok());
            }
        }
    }

    #[test]
    fn layout_and_aad_are_as_documented() {
        let rng = SeededRng::new(2);
        let bytes = seal_envelope(EnvelopeKind::Snapshot, b"hdr", &key(), b"data", None, &rng);
        assert_eq!(&bytes[..3], &[1, 1, 2]);
        assert_eq!(&bytes[3..7], &3u32.to_le_bytes());
        assert_eq!(&bytes[7..10], b"hdr");
        let r = regions(3, 4 + TAG_LEN);
        assert_eq!(&bytes[r.ciphertext_len.clone()], &20u32.to_le_bytes());
        assert_eq!(bytes.len(), r.ciphertext.end);

        let envelope = Envelope::decode(&bytes).unwrap();
        assert_eq!(envelope.aad(), [1, 1, 2, b'h', b'd', b'r']);
        let nonce = Nonce24::from_bytes(bytes[r.nonce].try_into().unwrap());
        let opened = aead::open(
            &key(),
            &nonce,
            &[1, 1, 2, b'h', b'd', b'r'],
            &bytes[r.ciphertext],
        );
        assert_eq!(opened.unwrap().as_slice(), b"data");
    }

    #[test]
    fn the_signature_covers_everything_before_it_with_the_kind_context() {
        let rng = SeededRng::new(3);
        let signing = signer();
        let bytes = seal_envelope(EnvelopeKind::Head, b"h", &key(), b"p", Some(&signing), &rng);
        let (body, signature) = bytes.split_at(bytes.len() - SIGNATURE_LEN);
        let signature = Signature::from_bytes(signature).unwrap();
        verify(&signing.verifying_key(), "envelope/head", body, &signature).unwrap();
        assert!(verify(&signing.verifying_key(), "envelope/entry", body, &signature).is_err());
    }

    #[test]
    fn tampering_with_any_region_fails() {
        let rng = SeededRng::new(4);
        let signing = signer();
        let verifier = signing.verifying_key();
        let header = b"12345";
        let plaintext = b"tamper target";
        let bytes = seal_envelope(
            EnvelopeKind::Entry,
            header,
            &key(),
            plaintext,
            Some(&signing),
            &rng,
        );
        let r = regions(header.len(), plaintext.len() + TAG_LEN);
        assert_eq!(bytes.len(), r.signature.end);

        let mut positions: Vec<(&str, usize)> = vec![
            ("version", 0),
            ("suite", 1),
            ("kind", 2),
            ("header length", 3),
            ("header length", 6),
        ];
        positions.extend(r.header.map(|i| ("header", i)));
        positions.extend(r.nonce.map(|i| ("nonce", i)));
        positions.extend(r.ciphertext_len.map(|i| ("ciphertext length", i)));
        positions.extend(r.ciphertext.map(|i| ("ciphertext", i)));
        positions.extend(r.signature.map(|i| ("signature", i)));

        for (region, i) in positions {
            for flip in [0x01u8, 0x80] {
                let mut tampered = bytes.clone();
                tampered[i] ^= flip;
                // With the verifier: always refused.
                assert!(
                    open_envelope(&tampered, &key(), Some(&verifier)).is_err(),
                    "{region} byte {i} flip {flip:#x} (verified)"
                );
                // Without it, every region except the signature is still protected.
                if region != "signature" {
                    assert!(
                        open_envelope(&tampered, &key(), None).is_err(),
                        "{region} byte {i} flip {flip:#x} (unverified)"
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_version_suite_or_kind_is_encoding() {
        let rng = SeededRng::new(5);
        let bytes = seal_envelope(EnvelopeKind::Chunk, b"", &key(), b"x", None, &rng);
        for (position, values, message) in [
            (0usize, [0u8, 2, 255], "unknown envelope version"),
            (1, [0, 2, 255], "unknown suite"),
            (2, [0, 12, 255], "unknown envelope kind"),
        ] {
            for value in values {
                let mut changed = bytes.clone();
                changed[position] = value;
                for result in [
                    Envelope::decode(&changed).map(|_| ()),
                    open_envelope(&changed, &key(), None).map(|_| ()),
                ] {
                    match result {
                        Err(CryptoError::Encoding(m)) => {
                            assert!(m.starts_with(message), "{m}");
                        }
                        other => panic!("byte {position} = {value}: {other:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn another_valid_kind_byte_fails_authentication() {
        let rng = SeededRng::new(6);
        let bytes = seal_envelope(EnvelopeKind::Keyring, b"h", &key(), b"x", None, &rng);
        let mut changed = bytes.clone();
        changed[2] = EnvelopeKind::Backup.to_byte();
        assert_eq!(
            open_envelope(&changed, &key(), None),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn truncation_and_trailing_bytes_are_encoding() {
        let rng = SeededRng::new(7);
        let signing = signer();
        let bytes = seal_envelope(
            EnvelopeKind::Entry,
            b"hh",
            &key(),
            b"pp",
            Some(&signing),
            &rng,
        );
        let unsigned_len = bytes.len() - SIGNATURE_LEN;
        for len in 0..bytes.len() {
            let result = Envelope::decode(&bytes[..len]);
            if len == unsigned_len {
                // Dropping the whole signature leaves a well-formed unsigned envelope;
                // a verifier then refuses it.
                assert!(result.unwrap().signature.is_none());
                assert_eq!(
                    open_envelope(&bytes[..len], &key(), Some(&signing.verifying_key())),
                    Err(CryptoError::Authentication)
                );
            } else {
                assert!(
                    matches!(result, Err(CryptoError::Encoding(_))),
                    "prefix of {len} bytes"
                );
            }
        }
        for extra in [1usize, 63, 65] {
            let mut longer = bytes[..unsigned_len].to_vec();
            longer.extend(std::iter::repeat_n(0u8, extra));
            assert!(matches!(
                Envelope::decode(&longer),
                Err(CryptoError::Encoding(_))
            ));
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(
            Envelope::decode(&longer),
            Err(CryptoError::Encoding(_))
        ));
    }

    #[test]
    fn a_ciphertext_shorter_than_the_tag_is_encoding() {
        let envelope = Envelope {
            kind: EnvelopeKind::Entry,
            header: Vec::new(),
            nonce: Nonce24::from_bytes([0; NONCE_LEN]),
            ciphertext: vec![0; TAG_LEN - 1],
            signature: None,
        };
        assert!(matches!(
            Envelope::decode(&envelope.encode()),
            Err(CryptoError::Encoding(_))
        ));
    }

    #[test]
    fn a_verifier_makes_the_signature_mandatory() {
        let rng = SeededRng::new(8);
        let unsigned = seal_envelope(EnvelopeKind::AccountDoc, b"", &key(), b"x", None, &rng);
        assert_eq!(
            open_envelope(&unsigned, &key(), Some(&signer().verifying_key())),
            Err(CryptoError::Authentication)
        );
        let signed = seal_envelope(
            EnvelopeKind::AccountDoc,
            b"",
            &key(),
            b"x",
            Some(&signer()),
            &rng,
        );
        let stranger = SigningKey::from_seed(&Secret32::from_bytes([0x33; 32]));
        assert_eq!(
            open_envelope(&signed, &key(), Some(&stranger.verifying_key())),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn wrong_key_is_authentication() {
        let rng = SeededRng::new(9);
        let bytes = seal_envelope(EnvelopeKind::Pairing, b"", &key(), b"x", None, &rng);
        assert_eq!(
            open_envelope(&bytes, &Secret32::from_bytes([0x12; 32]), None),
            Err(CryptoError::Authentication)
        );
    }
}
