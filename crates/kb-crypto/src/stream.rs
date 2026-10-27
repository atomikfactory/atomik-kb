//! Chunked streaming AEAD (`encryption-design.md`, "Attachments: chunked streaming
//! AEAD"): each chunk sealed with a random nonce and AAD = prefix || index (u64 LE) ||
//! is_final (u8), so chunks cannot be reordered, dropped, truncated or appended.
//! Used by attachments (M5) and the backup file (M4).
//!
//! A sealed chunk is `nonce (24) || ciphertext || tag (16)`. The reader must know
//! which chunk is the last one (the backup file and attachment manifests record the
//! chunk count); a mismatched flag fails authentication, and a stream that ends
//! before its final chunk is caught by [`StreamOpener::finish`].

use kb_types::Rng;
use zeroize::Zeroizing;

use crate::aead::{self, NONCE_LEN, Nonce24, TAG_LEN};
use crate::{CryptoError, Secret32};

/// Plaintext chunk size.
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Bytes a sealed chunk adds to its plaintext: nonce and tag.
pub const CHUNK_OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// `prefix || index (u64 LE) || is_final (u8)`. The suffix has a fixed length, so
/// the prefix is recovered unambiguously.
fn chunk_aad(prefix: &[u8], index: u64, is_final: bool) -> Vec<u8> {
    let mut aad = Vec::with_capacity(prefix.len() + 9);
    aad.extend_from_slice(prefix);
    aad.extend_from_slice(&index.to_le_bytes());
    aad.push(u8::from(is_final));
    aad
}

fn too_many_chunks() -> CryptoError {
    CryptoError::Encoding("the stream has too many chunks".to_owned())
}

/// Seals a sequence of chunks. Output of each call: `nonce || ciphertext || tag`.
pub struct StreamSealer {
    key: Secret32,
    aad_prefix: Vec<u8>,
    index: u64,
    finished: bool,
}

impl StreamSealer {
    /// A sealer for one stream; `aad_prefix` binds the stream to its context
    /// (attachment id, backup header hash, ...).
    #[must_use]
    pub fn new(key: Secret32, aad_prefix: Vec<u8>) -> Self {
        Self {
            key,
            aad_prefix,
            index: 0,
            finished: false,
        }
    }

    /// Seals the next chunk; `is_final` closes the stream (further calls are
    /// [`CryptoError::Encoding`]).
    ///
    /// Chunks may have any length (the format uses [`CHUNK_SIZE`] for all but the
    /// last); an empty final chunk is valid. The nonce is drawn from `rng`.
    pub fn seal_chunk(
        &mut self,
        plaintext: &[u8],
        is_final: bool,
        rng: &dyn Rng,
    ) -> Result<Vec<u8>, CryptoError> {
        if self.finished {
            return Err(CryptoError::Encoding(
                "the stream is already finished".to_owned(),
            ));
        }
        let next = self.index.checked_add(1).ok_or_else(too_many_chunks)?;
        let nonce = Nonce24::random(rng);
        let aad = chunk_aad(&self.aad_prefix, self.index, is_final);
        let ciphertext = aead::try_seal(&self.key, &nonce, &aad, plaintext)?;
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(nonce.as_bytes());
        out.extend_from_slice(&ciphertext);
        self.index = next;
        self.finished = is_final;
        Ok(out)
    }

    /// Index of the next chunk.
    #[must_use]
    pub const fn index(&self) -> u64 {
        self.index
    }

    /// `true` once the final chunk was sealed.
    #[must_use]
    pub const fn finished(&self) -> bool {
        self.finished
    }
}

/// Opens a sequence of chunks in order.
/// Opens one chunk of a stream by its position, without the sequential state of
/// [`StreamOpener`]: random access for readers that know which chunk they need (an
/// attachment byte range). The AAD binds `index` and `is_final` exactly as the sealer did,
/// so a chunk presented at the wrong position or with the wrong final flag fails.
pub fn open_chunk_at(
    key: &Secret32,
    aad_prefix: &[u8],
    index: u64,
    bytes: &[u8],
    is_final: bool,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let (nonce, ciphertext) = bytes
        .split_first_chunk::<NONCE_LEN>()
        .ok_or(CryptoError::Authentication)?;
    let aad = chunk_aad(aad_prefix, index, is_final);
    aead::open(key, &Nonce24::from_bytes(*nonce), &aad, ciphertext)
}

pub struct StreamOpener {
    key: Secret32,
    aad_prefix: Vec<u8>,
    index: u64,
    finished: bool,
}

impl StreamOpener {
    /// An opener for one stream (same key and prefix as the sealer).
    #[must_use]
    pub fn new(key: Secret32, aad_prefix: Vec<u8>) -> Self {
        Self {
            key,
            aad_prefix,
            index: 0,
            finished: false,
        }
    }

    /// Opens the next chunk; a chunk after the final one, or a flag that does not
    /// match what was sealed, fails.
    ///
    /// A chunk after the final one is [`CryptoError::Encoding`] (an extended
    /// stream). A chunk that does not authenticate at this position with this flag
    /// (wrong key or prefix, reordered, replayed, tampered or cut) is
    /// [`CryptoError::Authentication`]; the opener then stays where it was, but the
    /// caller is expected to abort the whole read.
    pub fn open_chunk(
        &mut self,
        bytes: &[u8],
        is_final: bool,
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        if self.finished {
            return Err(CryptoError::Encoding(
                "a chunk follows the final chunk".to_owned(),
            ));
        }
        let next = self.index.checked_add(1).ok_or_else(too_many_chunks)?;
        let plaintext = open_chunk_at(&self.key, &self.aad_prefix, self.index, bytes, is_final)?;
        self.index = next;
        self.finished = is_final;
        Ok(plaintext)
    }

    /// `true` once the final chunk was opened (a stream that ends without it was
    /// truncated).
    #[must_use]
    pub const fn finished(&self) -> bool {
        self.finished
    }

    /// Index of the next chunk.
    #[must_use]
    pub const fn index(&self) -> u64 {
        self.index
    }

    /// Call when the input ends: [`CryptoError::Authentication`] unless the final
    /// chunk was opened (the stream was truncated).
    pub fn finish(&self) -> Result<(), CryptoError> {
        if self.finished {
            Ok(())
        } else {
            Err(CryptoError::Authentication)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use kb_types::testing::SeededRng;

    fn key() -> Secret32 {
        Secret32::from_bytes([0x5c; 32])
    }

    const PREFIX: &[u8] = b"attachment:1234";

    fn seal_all(chunks: &[&[u8]], rng: &dyn Rng) -> Vec<Vec<u8>> {
        let mut sealer = StreamSealer::new(key(), PREFIX.to_vec());
        let last = chunks.len() - 1;
        chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| sealer.seal_chunk(chunk, i == last, rng).unwrap())
            .collect()
    }

    #[test]
    fn three_chunk_round_trip() {
        let rng = SeededRng::new(1);
        let chunks: [&[u8]; 3] = [b"first chunk", b"second", b""];
        let mut sealer = StreamSealer::new(key(), PREFIX.to_vec());
        let mut sealed = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(sealer.index(), i as u64);
            assert!(!sealer.finished());
            let bytes = sealer.seal_chunk(chunk, i == 2, &rng).unwrap();
            assert_eq!(bytes.len(), chunk.len() + CHUNK_OVERHEAD);
            sealed.push(bytes);
        }
        assert!(sealer.finished());
        assert_eq!(sealer.index(), 3);

        let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
        assert!(opener.finish().is_err());
        for (i, (bytes, chunk)) in sealed.iter().zip(chunks).enumerate() {
            let plaintext = opener.open_chunk(bytes, i == 2).unwrap();
            assert_eq!(plaintext.as_slice(), chunk);
        }
        assert!(opener.finished());
        assert_eq!(opener.index(), 3);
        opener.finish().unwrap();
    }

    #[test]
    fn the_aad_is_prefix_index_flag() {
        let rng = SeededRng::new(2);
        let sealed = seal_all(&[b"a", b"b"], &rng);
        let mut aad = PREFIX.to_vec();
        aad.extend_from_slice(&1u64.to_le_bytes());
        aad.push(1);
        let nonce = Nonce24::from_bytes(sealed[1][..NONCE_LEN].try_into().unwrap());
        let opened = aead::open(&key(), &nonce, &aad, &sealed[1][NONCE_LEN..]).unwrap();
        assert_eq!(opened.as_slice(), b"b");
    }

    #[test]
    fn reordering_or_replaying_fails() {
        let rng = SeededRng::new(3);
        let sealed = seal_all(&[b"0", b"1", b"2"], &rng);
        let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
        assert_eq!(
            opener.open_chunk(&sealed[1], false),
            Err(CryptoError::Authentication),
            "chunk 1 at position 0"
        );
        opener.open_chunk(&sealed[0], false).unwrap();
        assert_eq!(
            opener.open_chunk(&sealed[0], false),
            Err(CryptoError::Authentication),
            "chunk 0 replayed at position 1"
        );
        assert_eq!(
            opener.open_chunk(&sealed[2], true),
            Err(CryptoError::Authentication),
            "chunk 1 dropped"
        );
        // A failed chunk leaves the opener where it was.
        assert_eq!(opener.index(), 1);
        opener.open_chunk(&sealed[1], false).unwrap();
        opener.open_chunk(&sealed[2], true).unwrap();
        opener.finish().unwrap();
    }

    #[test]
    fn truncation_is_detected() {
        let rng = SeededRng::new(4);
        let sealed = seal_all(&[b"0", b"1", b"2"], &rng);
        let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
        opener.open_chunk(&sealed[0], false).unwrap();
        opener.open_chunk(&sealed[1], false).unwrap();
        // The input ends here: the final chunk never came.
        assert!(!opener.finished());
        assert_eq!(opener.finish(), Err(CryptoError::Authentication));
        // Claiming a non-final chunk is the last one does not work either.
        let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
        opener.open_chunk(&sealed[0], false).unwrap();
        assert_eq!(
            opener.open_chunk(&sealed[1], true),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn extension_is_refused() {
        let rng = SeededRng::new(5);
        let sealed = seal_all(&[b"0", b"1"], &rng);
        let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
        opener.open_chunk(&sealed[0], false).unwrap();
        opener.open_chunk(&sealed[1], true).unwrap();
        for (bytes, flag) in [(&sealed[0], false), (&sealed[1], true), (&sealed[1], false)] {
            assert!(matches!(
                opener.open_chunk(bytes, flag),
                Err(CryptoError::Encoding(_))
            ));
        }
        // A non-final chunk sealed as more of the stream does not pass as final.
        let mut sealer = StreamSealer::new(key(), PREFIX.to_vec());
        let first = sealer.seal_chunk(b"0", false, &rng).unwrap();
        let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
        assert_eq!(
            opener.open_chunk(&first, true),
            Err(CryptoError::Authentication),
            "a mismatched final flag fails authentication"
        );
    }

    #[test]
    fn the_sealer_refuses_chunks_after_the_final_one() {
        let rng = SeededRng::new(6);
        let mut sealer = StreamSealer::new(key(), PREFIX.to_vec());
        sealer.seal_chunk(b"only", true, &rng).unwrap();
        assert!(matches!(
            sealer.seal_chunk(b"more", false, &rng),
            Err(CryptoError::Encoding(_))
        ));
        assert!(matches!(
            sealer.seal_chunk(b"more", true, &rng),
            Err(CryptoError::Encoding(_))
        ));
        assert_eq!(sealer.index(), 1);
    }

    #[test]
    fn wrong_key_prefix_or_tampering_fails() {
        let rng = SeededRng::new(7);
        let sealed = seal_all(&[b"chunk"], &rng);
        let mut other_key = StreamOpener::new(Secret32::from_bytes([1; 32]), PREFIX.to_vec());
        assert_eq!(
            other_key.open_chunk(&sealed[0], true),
            Err(CryptoError::Authentication)
        );
        let mut other_prefix = StreamOpener::new(key(), b"attachment:1235".to_vec());
        assert_eq!(
            other_prefix.open_chunk(&sealed[0], true),
            Err(CryptoError::Authentication)
        );
        for i in 0..sealed[0].len() {
            let mut tampered = sealed[0].clone();
            tampered[i] ^= 0x20;
            let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
            assert_eq!(
                opener.open_chunk(&tampered, true),
                Err(CryptoError::Authentication),
                "byte {i}"
            );
        }
        for len in 0..sealed[0].len() {
            let mut opener = StreamOpener::new(key(), PREFIX.to_vec());
            assert_eq!(
                opener.open_chunk(&sealed[0][..len], true),
                Err(CryptoError::Authentication),
                "cut to {len} bytes"
            );
        }
    }

    #[test]
    fn chunk_size_is_four_mib() {
        assert_eq!(CHUNK_SIZE, 4_194_304);
        assert_eq!(CHUNK_OVERHEAD, 40);
    }
}
