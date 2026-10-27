//! Local AI rules (ADR-0037, `docs/architecture/ai.md`, section 3).
//!
//! Pure functions and types: Ollama's wire format and an NDJSON stream decoder, the prompts of
//! the writing assistant, the chat and the transcription, the chunking of a page for the semantic
//! index, vector math and reciprocal rank fusion, transcript cleanup and the endpoint policy. No
//! I/O, no clock, no state; compiles for `wasm32-unknown-unknown`. The shell does the HTTP calls,
//! the core keeps the index.

#![forbid(unsafe_code)]

pub mod chunk;
pub mod endpoint;
pub mod fusion;
pub mod ollama;
pub mod prompts;
pub mod transcript;
pub mod vector;

mod util;

/// Lower-case hex of bytes (hashes in the API are hex strings).
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// The bytes of a hex string (either case); `None` when its length is odd or a character is not
/// a hex digit. The inverse of [`hex`].
#[must_use]
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[high, low]| Some((hex_digit(*high)? << 4) | hex_digit(*low)?))
        .collect()
}

/// A 32-byte hash from its 64-character hex form (the `hash` and `sourceHash` of the API).
#[must_use]
pub fn hash_from_hex(text: &str) -> Option<[u8; 32]> {
    from_hex(text)?.try_into().ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(hex(&bytes), "000fa5ff");
        assert_eq!(from_hex("000fa5ff").unwrap(), bytes);
        assert_eq!(from_hex("000FA5FF").unwrap(), bytes);
        assert_eq!(from_hex("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn bad_hex_is_none() {
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("é1"), None);
    }

    #[test]
    fn a_hash_needs_exactly_32_bytes() {
        let hash = [7u8; 32];
        assert_eq!(hash_from_hex(&hex(&hash)), Some(hash));
        assert_eq!(hash_from_hex(&hex(&hash[..31])), None);
        assert_eq!(hash_from_hex(&format!("{}00", hex(&hash))), None);
    }
}
