//! Space key boxes, the backward chain and the derived keys.

#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::test_support::device;
use kb_crypto::{CryptoError, derive_key};
use kb_types::testing::SeededRng;

fn feed(byte: u8) -> FeedId {
    FeedId::from_bytes([byte; 16])
}

fn sk(byte: u8) -> SpaceKey {
    SpaceKey::from_secret(Secret32::from_bytes([byte; 32]))
}

fn is_authentication(error: &KeysError) -> bool {
    matches!(error, KeysError::Crypto(CryptoError::Authentication))
}

/// The key `00 01 .. 1f` of the cross-checked vectors.
fn ramp_key() -> SpaceKey {
    SpaceKey::from_secret(Secret32::from_bytes(core::array::from_fn(|i| {
        u8::try_from(i).unwrap()
    })))
}

/// The feed `10 11 .. 1f` of the cross-checked vectors.
fn ramp_feed() -> FeedId {
    FeedId::from_bytes(core::array::from_fn(|i| 0x10 + u8::try_from(i).unwrap()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_box_context_is_the_documented_string() {
    assert_eq!(
        space_key_context(ramp_feed(), 7),
        "atomik-kb/v1/space-key/101112131415161718191a1b1c1d1e1f/7"
    );
    // Decimal, no padding, the whole u32 range.
    assert_eq!(
        space_key_context(feed(0xab), 0),
        "atomik-kb/v1/space-key/abababababababababababababababab/0"
    );
    assert!(space_key_context(feed(0xab), u32::MAX).ends_with("/4294967295"));
    assert_ne!(
        space_key_context(feed(1), 12),
        space_key_context(feed(1), 1),
        "no ambiguity between epochs"
    );
}

#[test]
fn a_box_opens_for_its_device_only() {
    let rng = SeededRng::new(1);
    let (_, alice) = device(1);
    let (_, bob) = device(2);
    let key = sk(9);

    let sealed = seal_space_key_for(&key, feed(5), 3, &alice.public(), &rng);
    assert_eq!(
        sealed.len(),
        104,
        "ephemeral 32 + nonce 24 + key 32 + tag 16"
    );
    assert_eq!(
        open_space_key_box(&alice, feed(5), 3, &sealed).unwrap(),
        key
    );
    // Another device, feed or epoch.
    assert!(is_authentication(
        &open_space_key_box(&bob, feed(5), 3, &sealed).unwrap_err()
    ));
    assert!(is_authentication(
        &open_space_key_box(&alice, feed(6), 3, &sealed).unwrap_err()
    ));
    assert!(is_authentication(
        &open_space_key_box(&alice, feed(5), 4, &sealed).unwrap_err()
    ));
}

#[test]
fn a_box_is_sealed_with_the_documented_context() {
    // Spelled out: `open_sealed` with the literal context text, no helper of this module.
    let rng = SeededRng::new(2);
    let (_, alice) = device(1);
    let sealed = seal_space_key_for(&ramp_key(), ramp_feed(), 7, &alice.public(), &rng);
    let opened = kb_crypto::open_sealed(
        alice.agreement(),
        "atomik-kb/v1/space-key/101112131415161718191a1b1c1d1e1f/7",
        &sealed,
    )
    .unwrap();
    assert_eq!(opened.as_slice(), ramp_key().secret().expose());
}

#[test]
fn every_box_is_fresh_and_tampering_is_refused() {
    let rng = SeededRng::new(3);
    let (_, alice) = device(1);
    let a = seal_space_key_for(&sk(1), feed(1), 1, &alice.public(), &rng);
    let b = seal_space_key_for(&sk(1), feed(1), 1, &alice.public(), &rng);
    assert_ne!(a, b, "a fresh ephemeral key and nonce per box");
    for i in 0..a.len() {
        let mut bad = a.clone();
        bad[i] ^= 0x01;
        assert!(
            open_space_key_box(&alice, feed(1), 1, &bad).is_err(),
            "byte {i}"
        );
    }
    for len in [0, 10, 71, 103] {
        assert!(open_space_key_box(&alice, feed(1), 1, &a[..len]).is_err());
    }
}

#[test]
fn a_box_that_holds_something_else_than_a_key_is_refused() {
    // Anyone can seal to a public key; only a 32-byte plaintext is a key.
    let rng = SeededRng::new(4);
    let (_, alice) = device(1);
    for len in [0usize, 31, 33, 64] {
        let sealed = kb_crypto::seal_to(
            &alice.public().agreement,
            &space_key_context(feed(1), 1),
            &vec![7u8; len],
            &rng,
        );
        assert!(
            matches!(
                open_space_key_box(&alice, feed(1), 1, &sealed),
                Err(KeysError::Crypto(CryptoError::Encoding(_)))
            ),
            "{len} bytes"
        );
    }
}

#[test]
fn the_backward_chain_wraps_the_previous_key_under_the_new_one() {
    let rng = SeededRng::new(5);
    let (prev, new) = (sk(1), sk(2));
    let wrapped = wrap_prev(&prev, &new, feed(7), 4, &rng);
    assert_eq!(wrapped.to_bytes().len(), 72);
    assert_eq!(unwrap_prev(&new, &wrapped, feed(7), 4).unwrap(), prev);

    // The wrong key, feed or epoch.
    assert!(is_authentication(
        &unwrap_prev(&sk(3), &wrapped, feed(7), 4).unwrap_err()
    ));
    assert!(is_authentication(
        &unwrap_prev(&new, &wrapped, feed(8), 4).unwrap_err()
    ));
    assert!(is_authentication(
        &unwrap_prev(&new, &wrapped, feed(7), 5).unwrap_err()
    ));
    assert!(is_authentication(
        &unwrap_prev(&new, &wrapped, feed(7), 3).unwrap_err()
    ));
    // Tampering.
    let mut bytes = wrapped.to_bytes();
    bytes[30] ^= 1;
    let bad = WrappedKey::from_bytes(&bytes).unwrap();
    assert!(is_authentication(
        &unwrap_prev(&new, &bad, feed(7), 4).unwrap_err()
    ));
}

#[test]
fn the_backward_chain_uses_the_documented_subject() {
    // Spelled out: AAD subject = feed (16 bytes) then epoch_prev as a little-endian u32.
    let rng = SeededRng::new(6);
    let wrapped = wrap_prev(&ramp_key(), &sk(0x55), ramp_feed(), 258, &rng);
    let mut subject = ramp_feed().as_bytes().to_vec();
    subject.extend_from_slice(&[0x02, 0x01, 0x00, 0x00]);
    let opened = kb_crypto::unwrap_key(&wrapped, sk(0x55).secret(), &subject).unwrap();
    assert_eq!(&opened, ramp_key().secret());
}

#[test]
fn the_newest_key_opens_every_older_epoch() {
    let rng = SeededRng::new(7);
    let f = feed(2);
    let keys: Vec<SpaceKey> = (1u8..=5).map(sk).collect();
    // Epoch e holds keys[e - 1]; wraps[e - 1] wraps epoch e - 1's key under epoch e's.
    let wraps: Vec<WrappedKey> = (1..keys.len())
        .map(|i| wrap_prev(&keys[i - 1], &keys[i], f, u32::try_from(i).unwrap(), &rng))
        .collect();
    let mut current = keys[4].clone();
    for i in (1..keys.len()).rev() {
        current = unwrap_prev(&current, &wraps[i - 1], f, u32::try_from(i).unwrap()).unwrap();
        assert_eq!(current, keys[i - 1], "epoch {i}");
    }
    // A key cannot open an epoch that is not its direct predecessor.
    assert!(unwrap_prev(&keys[4], &wraps[0], f, 1).is_err());
}

#[test]
fn derived_keys_follow_the_documented_derivations() {
    let key = ramp_key();
    let feed = ramp_feed();
    assert_eq!(
        membership_meta_key(&key, feed),
        derive_key(key.secret(), Label::MEMBERSHIP_META, feed.as_bytes())
    );
    assert_eq!(
        space_keyring_key(&key, feed),
        derive_key(key.secret(), Label::SPACE_KEYRING, feed.as_bytes())
    );
    // Cross-checked against RFC 5869 HKDF-SHA256 written out with HMAC in Python
    // (salt "atomik-kb/v1", info "atomik-kb/v1/<label>" 00 <feed bytes>).
    assert_eq!(
        hex(membership_meta_key(&key, feed).expose()),
        "bbe1145a609cc7b03c324a8a98a462e6e224be0ae2c109bae4d3acfdc6a20b38"
    );
    assert_eq!(
        hex(space_keyring_key(&key, feed).expose()),
        "d8b04e8d10a3b321ea1fe8bcb5b78cc1f38a7cf31075539f9744c3025dfd3ad6"
    );
    // Slice 3: the presence key, same cross-check.
    assert_eq!(
        presence_key(&key, feed),
        derive_key(key.secret(), Label::PRESENCE, feed.as_bytes())
    );
    assert_eq!(
        hex(presence_key(&key, feed).expose()),
        "18f688044d8f4b959b3180d73917c024b61b5954bde966f35d60975ab7bbbee3"
    );
}

#[test]
fn the_presence_key_is_bound_to_the_key_the_feed_and_the_purpose() {
    let (a, b) = (sk(1), sk(2));
    assert_ne!(presence_key(&a, feed(1)), presence_key(&b, feed(1)));
    assert_ne!(presence_key(&a, feed(1)), presence_key(&a, feed(2)));
    assert_ne!(presence_key(&a, feed(1)), space_keyring_key(&a, feed(1)));
    assert_ne!(presence_key(&a, feed(1)), membership_meta_key(&a, feed(1)));
}

#[test]
fn derived_keys_are_bound_to_the_key_the_feed_and_the_purpose() {
    let (a, b) = (sk(1), sk(2));
    assert_ne!(
        membership_meta_key(&a, feed(1)),
        membership_meta_key(&b, feed(1))
    );
    assert_ne!(
        membership_meta_key(&a, feed(1)),
        membership_meta_key(&a, feed(2))
    );
    assert_ne!(
        space_keyring_key(&a, feed(1)),
        space_keyring_key(&a, feed(2))
    );
    assert_ne!(
        membership_meta_key(&a, feed(1)),
        space_keyring_key(&a, feed(1)),
        "two purposes, two keys"
    );
    assert_ne!(
        membership_meta_key(&a, feed(1)).expose(),
        a.secret().expose()
    );
}

#[test]
fn the_commitment_is_the_one_of_kb_crypto() {
    let key = ramp_key();
    assert_eq!(
        key.commitment(ramp_feed(), 258),
        kb_crypto::sk_commitment(ramp_feed(), 258, key.secret())
    );
    // The vector of kb-crypto, cross-checked there with an independent BLAKE3.
    assert_eq!(
        hex(&key.commitment(ramp_feed(), 258)),
        "834df1723bad17662814de3b1ce699da0d10f92d845e94782496e8ecbd5dd60a"
    );
}

#[test]
fn a_space_key_never_prints_its_bytes() {
    let text = format!("{:?}", sk(0xAB));
    assert_eq!(text, "SpaceKey(Secret32(<redacted>))");
}

#[test]
fn generate_draws_from_the_rng() {
    let a = SpaceKey::generate(&SeededRng::new(1));
    let b = SpaceKey::generate(&SeededRng::new(1));
    let c = SpaceKey::generate(&SeededRng::new(2));
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.secret(), &Secret32::generate(&SeededRng::new(1)));
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // A box costs two scalar multiplications; the debug build of the curve crates is slow.
        #![proptest_config(ProptestConfig::with_cases(12))]

        #[test]
        fn a_box_opens_for_its_own_feed_epoch_and_device_only(
            key in any::<[u8; 32]>(),
            feed_bytes in any::<[u8; 16]>(),
            epoch in any::<u32>(),
            seed in any::<u64>(),
        ) {
            let rng = SeededRng::new(seed);
            let (_, alice) = device(1);
            let (_, bob) = device(2);
            let key = SpaceKey::from_secret(Secret32::from_bytes(key));
            let feed = FeedId::from_bytes(feed_bytes);
            let sealed = seal_space_key_for(&key, feed, epoch, &alice.public(), &rng);
            prop_assert_eq!(open_space_key_box(&alice, feed, epoch, &sealed).unwrap(), key);
            prop_assert!(open_space_key_box(&bob, feed, epoch, &sealed).is_err());
            prop_assert!(open_space_key_box(&alice, feed, epoch.wrapping_add(1), &sealed).is_err());
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn the_backward_chain_round_trips_and_is_bound_to_feed_and_epoch(
            prev in any::<[u8; 32]>(),
            new in any::<[u8; 32]>(),
            feed_bytes in any::<[u8; 16]>(),
            epoch_prev in any::<u32>(),
            seed in any::<u64>(),
        ) {
            let rng = SeededRng::new(seed);
            let prev = SpaceKey::from_secret(Secret32::from_bytes(prev));
            let new = SpaceKey::from_secret(Secret32::from_bytes(new));
            let feed = FeedId::from_bytes(feed_bytes);
            let wrapped = wrap_prev(&prev, &new, feed, epoch_prev, &rng);
            prop_assert_eq!(unwrap_prev(&new, &wrapped, feed, epoch_prev).unwrap(), prev);
            prop_assert!(unwrap_prev(&new, &wrapped, feed, epoch_prev.wrapping_add(1)).is_err());
            let mut other = feed_bytes;
            other[0] ^= 1;
            prop_assert!(unwrap_prev(&new, &wrapped, FeedId::from_bytes(other), epoch_prev).is_err());
        }
    }
}
