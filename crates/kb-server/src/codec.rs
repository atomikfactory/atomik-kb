//! Text encodings of the JSON surface: lowercase hex (epochs, nonces, signatures,
//! chain hashes, chunk ids) and standard base64 with padding (RFC 4648 §4: device log
//! entries and heads inside JSON). Small and strict enough for the server's needs, so
//! the crate carries no extra dependency for them.

const HEX: &[u8; 16] = b"0123456789abcdef";
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Hex of either case into exactly `N` bytes.
pub(crate) fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    let (pairs, _) = bytes.as_chunks::<2>();
    for (slot, [high, low]) in out.iter_mut().zip(pairs) {
        *slot = (hex_value(*high)? << 4) | hex_value(*low)?;
    }
    Some(out)
}

/// Standard base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(char::from(B64[usize::from(b0 >> 2)]));
        out.push(char::from(B64[usize::from(((b0 & 0x03) << 4) | (b1 >> 4))]));
        if chunk.len() > 1 {
            out.push(char::from(B64[usize::from(((b1 & 0x0f) << 2) | (b2 >> 6))]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(B64[usize::from(b2 & 0x3f)]));
        } else {
            out.push('=');
        }
    }
    out
}

fn b64_value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Standard base64; padding is optional, anything else outside the alphabet (and
/// non-zero trailing bits) is refused.
pub(crate) fn unbase64(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim_end_matches('=');
    if text.len() - trimmed.len() > 2 {
        return None;
    }
    let input = trimmed.as_bytes();
    if input.len() % 4 == 1 {
        return None;
    }
    if text.len() != trimmed.len() && !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 2);
    for chunk in input.chunks(4) {
        let mut acc: u32 = 0;
        for &c in chunk {
            acc = (acc << 6) | u32::from(b64_value(c)?);
        }
        let [_, a, b, c] = match chunk.len() {
            4 => acc,
            3 => {
                if acc & 0x03 != 0 {
                    return None;
                }
                acc << 6
            }
            2 => {
                if acc & 0x0f != 0 {
                    return None;
                }
                acc << 12
            }
            _ => return None,
        }
        .to_be_bytes();
        out.push(a);
        if chunk.len() > 2 {
            out.push(b);
        }
        if chunk.len() > 3 {
            out.push(c);
        }
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_refuses_bad_input() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
        assert_eq!(unhex::<3>("00ABff").unwrap(), [0x00, 0xab, 0xff]);
        assert!(unhex::<3>("00abf").is_none());
        assert!(unhex::<2>("00abff").is_none());
        assert!(unhex::<1>("zz").is_none());
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(base64(plain.as_bytes()), encoded);
            assert_eq!(unbase64(encoded).unwrap(), plain.as_bytes());
            assert_eq!(
                unbase64(encoded.trim_end_matches('=')).unwrap(),
                plain.as_bytes()
            );
        }
    }

    #[test]
    fn base64_round_trips_every_byte() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        for len in 0..64 {
            assert_eq!(unbase64(&base64(&bytes[..len])).unwrap(), &bytes[..len]);
        }
        assert_eq!(unbase64(&base64(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn base64_refuses_malformed_input() {
        for bad in [
            "Z", "Zg===", "Zh==", "Zm9=", "Zm9v!", "Zm 9v", "Zg=", "=Zg=",
        ] {
            assert!(unbase64(bad).is_none(), "{bad}");
        }
    }
}
