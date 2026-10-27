//! Size-bucket padding (`encryption-design.md`, "Padding"): 256-byte steps up to
//! 4 KiB, then powers of two. Padding bytes are random; the plaintext carries its
//! own length.
//!
//! Layout: `u32 LE length || plaintext || random padding`, the whole of it exactly
//! one bucket long. Buckets: 256, 512, ..., 3840, 4096, then 8192, 16384, ...

use kb_types::Rng;

use crate::CryptoError;

/// Length prefix size.
pub const LENGTH_PREFIX_LEN: usize = 4;

/// Bucket step below [`LINEAR_BUCKET_LIMIT`]; also the smallest bucket.
pub const BUCKET_STEP: usize = 256;
/// Largest bucket of the linear range; above it buckets are powers of two.
pub const LINEAR_BUCKET_LIMIT: usize = 4096;

/// [`bucket_size`] without the saturation: `None` when no bucket fits in `usize`.
fn checked_bucket_size(len: usize) -> Option<usize> {
    if len <= LINEAR_BUCKET_LIMIT {
        Some(len.div_ceil(BUCKET_STEP).max(1) * BUCKET_STEP)
    } else {
        len.checked_next_power_of_two()
    }
}

/// The bucket a payload of `len` bytes (prefix included) is padded to: the next
/// multiple of 256 (at least 256) up to 4096, then the next power of two.
///
/// Lengths above the largest power of two of `usize` have no bucket; the result is
/// then `usize::MAX`, which is not a bucket (unreachable for real payloads).
#[must_use]
pub fn bucket_size(len: usize) -> usize {
    checked_bucket_size(len).unwrap_or(usize::MAX)
}

/// `true` when `len` is exactly a bucket size.
fn is_bucket(len: usize) -> bool {
    checked_bucket_size(len) == Some(len)
}

/// `u32 LE length || plaintext || random padding` to the next bucket.
///
/// A plaintext longer than `u32::MAX` bytes (or without a bucket) cannot be framed;
/// the result is then empty, which [`unpad`] refuses (closed failure; unreachable on
/// wasm32 and for any payload this application pads).
#[must_use]
pub fn pad_to_bucket(plaintext: &[u8], rng: &dyn Rng) -> Vec<u8> {
    let Ok(len) = u32::try_from(plaintext.len()) else {
        return Vec::new();
    };
    let Some(total) = plaintext
        .len()
        .checked_add(LENGTH_PREFIX_LEN)
        .and_then(checked_bucket_size)
    else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(plaintext);
    let content_len = out.len();
    out.resize(total, 0);
    if let Some(padding) = out.get_mut(content_len..) {
        rng.fill_bytes(padding);
    }
    out
}

/// Reverses [`pad_to_bucket`]; an inconsistent length is [`CryptoError::Encoding`]:
/// the input must be exactly one bucket long and the stated length must fit in it.
/// The padding bytes themselves are not checked (they are random).
pub fn unpad(padded: &[u8]) -> Result<&[u8], CryptoError> {
    if !is_bucket(padded.len()) {
        return Err(CryptoError::Encoding(format!(
            "{} bytes is not a padding bucket size",
            padded.len()
        )));
    }
    let (prefix, rest) = padded
        .split_first_chunk::<LENGTH_PREFIX_LEN>()
        .ok_or_else(|| CryptoError::Encoding("missing length prefix".to_owned()))?;
    let len = usize::try_from(u32::from_le_bytes(*prefix))
        .map_err(|_| CryptoError::Encoding("padded length out of range".to_owned()))?;
    rest.get(..len).ok_or_else(|| {
        CryptoError::Encoding(format!(
            "stated length {len} exceeds the {}-byte bucket",
            padded.len()
        ))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use kb_types::testing::SeededRng;

    #[test]
    fn bucket_table() {
        let table = [
            (0usize, 256usize),
            (1, 256),
            (4, 256),
            (255, 256),
            (256, 256),
            (257, 512),
            (511, 512),
            (512, 512),
            (513, 768),
            (3840, 3840),
            (3841, 4096),
            (4095, 4096),
            (4096, 4096),
            (4097, 8192),
            (8191, 8192),
            (8192, 8192),
            (8193, 16_384),
            (65_536, 65_536),
            (65_537, 131_072),
            (1 << 20, 1 << 20),
            ((1 << 20) + 1, 1 << 21),
            (4 * 1024 * 1024 + 4, 8 * 1024 * 1024),
        ];
        for (len, bucket) in table {
            assert_eq!(bucket_size(len), bucket, "len {len}");
            assert!(is_bucket(bucket), "{bucket} is a bucket");
        }
        assert_eq!(bucket_size(usize::MAX), usize::MAX);
        assert_eq!(bucket_size((usize::MAX >> 1) + 2), usize::MAX);
    }

    #[test]
    fn the_buckets_are_the_documented_ones() {
        let linear: Vec<usize> = (1..=16).map(|i| i * 256).collect();
        for n in 1..=LINEAR_BUCKET_LIMIT {
            assert_eq!(is_bucket(n), linear.contains(&n), "{n}");
        }
        for shift in 13..30 {
            assert!(is_bucket(1 << shift));
            assert!(!is_bucket((1 << shift) - 256));
            assert!(!is_bucket((1 << shift) + 256));
        }
    }

    #[test]
    fn pad_layout_and_round_trip() {
        let rng = SeededRng::new(1);
        for len in [0usize, 1, 100, 252, 253, 1000, 4092, 4093, 5000, 70_000] {
            let plaintext: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
            let padded = pad_to_bucket(&plaintext, &rng);
            assert_eq!(
                padded.len(),
                bucket_size(len + LENGTH_PREFIX_LEN),
                "len {len}"
            );
            assert_eq!(&padded[..4], &u32::try_from(len).unwrap().to_le_bytes());
            assert_eq!(&padded[4..4 + len], plaintext.as_slice());
            assert_eq!(unpad(&padded).unwrap(), plaintext.as_slice());
        }
    }

    #[test]
    fn padding_bytes_are_random() {
        let rng = SeededRng::new(2);
        let a = pad_to_bucket(b"short", &rng);
        let b = pad_to_bucket(b"short", &rng);
        assert_eq!(a.len(), 256);
        assert_ne!(a[9..], b[9..], "padding drawn from the rng");
        assert!(a[9..].iter().any(|&byte| byte != 0), "not zero padding");
    }

    #[test]
    fn unpad_refuses_inconsistent_input() {
        for len in [0usize, 3, 4, 255, 257, 300, 4097, 5000] {
            assert!(
                matches!(unpad(&vec![0u8; len]), Err(CryptoError::Encoding(_))),
                "{len} bytes"
            );
        }
        // A stated length that does not fit in the bucket.
        let mut padded = vec![0u8; 256];
        padded[..4].copy_from_slice(&253u32.to_le_bytes());
        assert!(matches!(unpad(&padded), Err(CryptoError::Encoding(_))));
        padded[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(unpad(&padded), Err(CryptoError::Encoding(_))));
        // The largest that fits.
        padded[..4].copy_from_slice(&252u32.to_le_bytes());
        assert_eq!(unpad(&padded).unwrap().len(), 252);
        // A larger bucket than needed is still accepted (the length is authoritative).
        let mut roomy = vec![0u8; 512];
        roomy[..4].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(unpad(&roomy).unwrap(), &[0, 0, 0]);
    }
}
