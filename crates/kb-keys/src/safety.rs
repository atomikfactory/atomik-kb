//! Safety numbers (M13, ADR-0042, `docs/security/sharing.md` section 2,
//! `docs/development/milestone-13.md` section 1.2): the 30 digits two people compare to be sure
//! each talks to the account they think.
//!
//! `G` is the full 32-byte BLAKE3 of an account's genesis entry (its `LogEntry::encode` bytes), so
//! the account id is the first 16 bytes of `G`. The number is
//! `BLAKE3(SAFETY_NUMBER_DOMAIN ‖ min(Ga, Gb) ‖ max(Ga, Gb))`, `min` and `max` comparing the two
//! 32-byte values as byte strings; the first 13 bytes read as a big-endian unsigned integer,
//! reduced mod 10^30 and zero-padded to 30 digits, shown as six groups of five separated by a
//! single space. It is stable across new devices, kit recoveries and rotations: it covers only the
//! genesis of each account.
//!
use kb_types::sharing::SAFETY_NUMBER_DOMAIN;

use crate::log::DeviceLog;

/// Digits of a safety number.
pub const SAFETY_NUMBER_DIGITS: usize = 30;
/// Digits per group.
pub const SAFETY_NUMBER_GROUP: usize = 5;

/// `G`: the BLAKE3 of the log's genesis entry (`LogEntry::hash` of entry 0). A verified log is never
/// empty; for an empty one the answer is 32 zero bytes.
#[must_use]
pub fn genesis_hash(log: &DeviceLog) -> [u8; 32] {
    log.entries()
        .first()
        .map_or([0u8; 32], crate::log::LogEntry::hash)
}

/// The safety number of two accounts from their genesis hashes: 30 digits in six groups of five,
/// `"12345 67890 12345 67890 12345 67890"`. Symmetric: `safety_number(a, b) == safety_number(b, a)`.
///
/// `BLAKE3(SAFETY_NUMBER_DOMAIN ‖ min(Ga, Gb) ‖ max(Ga, Gb))`; the first 13 bytes (104 bits) read as
/// a **big-endian** unsigned integer, reduced mod 10^30 (which is below 2^100, so every digit
/// string is reachable and the bias is negligible), zero-padded on the left to 30 digits.
#[must_use]
pub fn safety_number(ga: &[u8; 32], gb: &[u8; 32]) -> String {
    let (low, high) = if ga <= gb { (ga, gb) } else { (gb, ga) };
    let mut hasher = kb_crypto::Hasher::new();
    hasher.update(SAFETY_NUMBER_DOMAIN.as_bytes());
    hasher.update(low);
    hasher.update(high);
    let digest = hasher.finalize();

    // 13 big-endian bytes right-aligned in a u128.
    let mut wide = [0u8; 16];
    wide[3..].copy_from_slice(&digest[..13]);
    let value = u128::from_be_bytes(wide) % MODULUS;

    let digits = format!("{value:0width$}", width = SAFETY_NUMBER_DIGITS);
    group(&digits)
}

/// `10^30`.
const MODULUS: u128 = 1_000_000_000_000_000_000_000_000_000_000;

// The modulus and the digit count agree.
const _: () = assert!(SAFETY_NUMBER_DIGITS == 30 && MODULUS == 10u128.pow(30));
const _: () = assert!(SAFETY_NUMBER_DIGITS.is_multiple_of(SAFETY_NUMBER_GROUP));

/// `digits` in groups of [`SAFETY_NUMBER_GROUP`] separated by one space.
fn group(digits: &str) -> String {
    digits
        .as_bytes()
        .chunks(SAFETY_NUMBER_GROUP)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
