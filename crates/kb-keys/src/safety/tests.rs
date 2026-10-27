//! The safety number and the genesis hash.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::log::{DeviceLog, LogSigner, account_id_of};
use crate::test_support::device;
use kb_crypto::{Secret32, SigningKey};

const T0: i64 = 1_760_000_000_000;

fn ramp(start: u8) -> [u8; 32] {
    core::array::from_fn(|i| start.wrapping_add(u8::try_from(i).unwrap()))
}

/// The first 13 bytes of the digest as a decimal number, by schoolbook arithmetic on decimal
/// digits (no `u128`, no modulus operator): `value = value * 256 + byte` per byte, then the last 30
/// digits. An independent way to the same answer as `safety_number`.
fn decimal_of_first_13_bytes(digest: &[u8; 32]) -> String {
    let mut digits: Vec<u32> = vec![0]; // least significant digit first
    for byte in &digest[..13] {
        let mut carry = u32::from(*byte);
        for digit in &mut digits {
            let product = *digit * 256 + carry;
            *digit = product % 10;
            carry = product / 10;
        }
        while carry > 0 {
            digits.push(carry % 10);
            carry /= 10;
        }
    }
    digits.resize(digits.len().max(30), 0);
    digits
        .iter()
        .take(30)
        .rev()
        .map(|d| char::from_digit(*d, 10).unwrap())
        .collect()
}

fn spaced(digits: &str) -> String {
    (0..6)
        .map(|g| &digits[g * 5..g * 5 + 5])
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_number_matches_the_cross_checked_vectors() {
    // Ga = 00 01 .. 1f, Gb = 20 21 .. 3f. Computed with an independent BLAKE3 (a Python
    // implementation of the reference algorithm, checked against the official vectors for
    // the empty and one-byte inputs) and Python integers; the first 13 digest bytes are
    // a649309c4b0db130f7149f6e6b.
    let (ga, gb) = (ramp(0), ramp(32));
    assert_eq!(
        safety_number(&ga, &gb),
        "17452 61495 97843 48956 95922 74539"
    );
    // Symmetric.
    assert_eq!(
        safety_number(&gb, &ga),
        "17452 61495 97843 48956 95922 74539"
    );
    // The extremes of the byte order.
    assert_eq!(
        safety_number(&[0xff; 32], &[0; 32]),
        "57534 20659 62523 50554 89340 27256"
    );
}

#[test]
fn a_small_value_is_zero_padded_on_the_left() {
    // Gb = 0xaa.. and Ga = 0xa9.. : the reduced value is 1.5e27, 28 digits.
    assert_eq!(
        safety_number(&[169; 32], &[170; 32]),
        "00152 47279 70817 95732 80608 94907"
    );
}

#[test]
fn the_integer_is_big_endian_and_the_formula_is_spelled_out() {
    for (ga, gb) in [
        (ramp(0), ramp(32)),
        ([9; 32], [3; 32]),
        ([0xff; 32], [0; 32]),
        (ramp(7), ramp(7)),
    ] {
        // BLAKE3("atomik-kb/v1/safety-number" ‖ min ‖ max), by hand.
        let (low, high) = if ga <= gb { (ga, gb) } else { (gb, ga) };
        let mut input = b"atomik-kb/v1/safety-number".to_vec();
        input.extend_from_slice(&low);
        input.extend_from_slice(&high);
        assert_eq!(input.len(), 26 + 64);
        let digest = kb_crypto::hash(&input);

        let expected = spaced(&decimal_of_first_13_bytes(&digest));
        assert_eq!(safety_number(&ga, &gb), expected);
        assert_eq!(safety_number(&gb, &ga), expected, "symmetric");
    }
    // Reading the same 13 bytes little-endian gives another number: the order is fixed.
    let digest = kb_crypto::hash(&{
        let mut input = b"atomik-kb/v1/safety-number".to_vec();
        input.extend_from_slice(&ramp(0));
        input.extend_from_slice(&ramp(32));
        input
    });
    let mut reversed = [0u8; 32];
    for (i, byte) in digest[..13].iter().rev().enumerate() {
        reversed[i] = *byte;
    }
    assert_ne!(
        spaced(&decimal_of_first_13_bytes(&reversed)),
        safety_number(&ramp(0), &ramp(32))
    );
}

#[test]
fn the_shape_is_six_groups_of_five_digits() {
    for seed in 0u8..40 {
        let number = safety_number(&ramp(seed), &ramp(seed.wrapping_mul(3).wrapping_add(1)));
        let groups: Vec<&str> = number.split(' ').collect();
        assert_eq!(groups.len(), 6, "{number}");
        for group in groups {
            assert_eq!(group.len(), 5, "{number}");
            assert!(group.bytes().all(|b| b.is_ascii_digit()), "{number}");
        }
        assert_eq!(number.len(), SAFETY_NUMBER_DIGITS + 5);
    }
    assert_eq!(SAFETY_NUMBER_DIGITS, 30);
    assert_eq!(SAFETY_NUMBER_GROUP, 5);
}

#[test]
fn the_number_depends_on_both_accounts_and_nothing_else() {
    let (a, b, c) = (ramp(1), ramp(2), ramp(3));
    assert_ne!(safety_number(&a, &b), safety_number(&a, &c));
    assert_ne!(safety_number(&a, &b), safety_number(&c, &b));
    assert_eq!(safety_number(&a, &b), safety_number(&a, &b));
    let mut flipped = b;
    flipped[31] ^= 1;
    assert_ne!(safety_number(&a, &b), safety_number(&a, &flipped));
}

fn log_of(seed: u64) -> (DeviceLog, crate::DeviceKeys, kb_types::DeviceId) {
    let (id, keys) = device(seed);
    let kit = SigningKey::from_seed(&Secret32::from_bytes([7; 32]));
    let log = DeviceLog::genesis(id, &keys, kit.verifying_key(), vec![1; 12], T0).unwrap();
    (log, keys, id)
}

#[test]
fn the_genesis_hash_is_the_blake3_of_the_genesis_entry() {
    let (log, _, _) = log_of(1);
    let genesis = &log.entries()[0];
    assert_eq!(genesis_hash(&log), kb_crypto::hash(&genesis.encode()));
    // The account id is its first 16 bytes (ADR-0042).
    assert_eq!(account_id_of(genesis).as_bytes(), &genesis_hash(&log)[..16]);
    assert_ne!(genesis_hash(&log), [0u8; 32]);
}

#[test]
fn the_genesis_hash_is_stable_across_devices_and_revocations() {
    let (mut log, keys, id) = log_of(1);
    let before = genesis_hash(&log);
    let (other_id, other) = device(2);
    log.add_device(
        other_id,
        other.public(),
        vec![2; 12],
        LogSigner::Device(&keys, id),
        T0 + 1,
    )
    .unwrap();
    log.revoke_device(id, (&other, other_id), T0 + 2).unwrap();
    assert_eq!(
        genesis_hash(&log),
        before,
        "a device change keeps the number"
    );
}

#[test]
fn two_accounts_have_two_genesis_hashes_and_an_empty_log_none() {
    let (a, _, _) = log_of(1);
    let (b, _, _) = log_of(2);
    assert_ne!(genesis_hash(&a), genesis_hash(&b));
    assert_eq!(genesis_hash(&DeviceLog::default()), [0u8; 32]);
    // The number of a pair of real accounts has the documented shape.
    let number = safety_number(&genesis_hash(&a), &genesis_hash(&b));
    assert_eq!(number.split(' ').count(), 6);
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        #[test]
        fn the_number_is_symmetric_and_thirty_digits(a in any::<[u8; 32]>(), b in any::<[u8; 32]>()) {
            let number = safety_number(&a, &b);
            prop_assert_eq!(&number, &safety_number(&b, &a));
            let digits: String = number.split(' ').collect();
            prop_assert_eq!(digits.len(), 30);
            prop_assert!(digits.bytes().all(|c| c.is_ascii_digit()));
            prop_assert_eq!(number.split(' ').count(), 6);
            // The same thirty digits as the schoolbook computation in the unit tests.
            let (low, high) = if a <= b { (a, b) } else { (b, a) };
            let mut input = b"atomik-kb/v1/safety-number".to_vec();
            input.extend_from_slice(&low);
            input.extend_from_slice(&high);
            let digest = kb_crypto::hash(&input);
            prop_assert_eq!(number, spaced(&decimal_of_first_13_bytes(&digest)));
        }
    }
}
