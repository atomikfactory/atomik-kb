//! The codec of protocol v2: exact byte layouts (golden), round trips, tampering of every region,
//! v1 objects refused, the feed item helpers, and the bodies behind a valid signature.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::{Envelope, EnvelopeKind, SIGNATURE_LEN, Secret32, SigningKey};
use kb_keys::keyring::{KeySubject, new_space_key, tombstone};
use kb_keys::{Ark, DeviceKeys, KeyringItem};
use kb_types::sync::KEYRING_DOC_ID;
use kb_types::testing::SeededRng;
use kb_types::{AccountId, DeviceId, DocId, FeedId, SpaceId};

use super::*;
use crate::v2::conformance::{FIXTURE_TIME_MS, Person, Space};
use crate::{EntryBody, SnapshotBody};

fn key(byte: u8) -> Secret32 {
    Secret32::from_bytes([byte; 32])
}

fn signer(byte: u8) -> SigningKey {
    SigningKey::from_seed(&key(byte))
}

fn entry_header() -> EntryHeader {
    EntryHeader {
        feed: FeedId::from_bytes([0xf1; 16]),
        account_id: AccountId::from_bytes([0xa1; 16]),
        doc_id: DocId::from_bytes([0xd1; 16]),
        device_id: DeviceId::from_bytes([0xe1; 16]),
        device_seq: 0x0102_0304_0506_0708,
        key_epoch: 0x0a0b_0c0d,
    }
}

fn entry_body() -> EntryBody {
    EntryBody {
        engine: 1,
        format_version: 0x0201,
        min_reader_version: 0x0403,
        doc_kind: 3,
        updates: vec![b"first update".to_vec(), Vec::new(), vec![0xff; 300]],
        version_vector: b"vv".to_vec(),
    }
}

fn keyring_header() -> KeyringHeader {
    KeyringHeader {
        feed: FeedId::from_bytes([0xf2; 16]),
        account_id: AccountId::from_bytes([0xa2; 16]),
        device_id: DeviceId::from_bytes([0xe2; 16]),
        device_seq: 0x1112_1314_1516_1718,
        key_epoch: 7,
    }
}

fn snapshot_header() -> SnapshotHeader {
    SnapshotHeader {
        feed: FeedId::from_bytes([0xf3; 16]),
        account_id: AccountId::from_bytes([0xa3; 16]),
        doc_id: DocId::from_bytes([0xd3; 16]),
        device_id: DeviceId::from_bytes([0xe3; 16]),
        covers_cursor: 0x2122_2324_2526_2728,
        key_epoch: 9,
    }
}

fn snapshot_body() -> SnapshotBody {
    SnapshotBody {
        engine: 1,
        format_version: 1,
        min_reader_version: 1,
        doc_kind: 2,
        snapshot: vec![0x42; 5000],
        version_vector: b"version vector".to_vec(),
    }
}

fn head() -> Head {
    Head {
        feed: FeedId::from_bytes([0xf4; 16]),
        account_id: AccountId::from_bytes([0xa4; 16]),
        device_id: DeviceId::from_bytes([0xe4; 16]),
        cursor: 0x3132_3334_3536_3738,
        chain_hash: [0xc4; 32],
        server_epoch: [0x5e; 16],
        at_ms: 0x0102_0304_0506_0708,
    }
}

fn keyring_items(rng: &SeededRng) -> Vec<KeyringItem> {
    let ark = Ark::from_secret(key(0x77));
    let device = DeviceKeys::generate(rng);
    let device_id = DeviceId::generate(rng);
    let space = SpaceId::generate(rng);
    let (_, record) = new_space_key(&ark, space, 0, &device, device_id, 1_000, rng);
    let gone = tombstone(
        space,
        KeySubject::Doc {
            doc_id: DocId::generate(rng),
            epoch: 0,
        },
        &device,
        device_id,
        2_000,
    );
    vec![KeyringItem::Record(record), KeyringItem::Tombstone(gone)]
}

#[test]
fn header_lengths_are_those_of_adr_0045() {
    assert_eq!(ENTRY_HEADER_LEN, 77);
    assert_eq!(KEYRING_HEADER_LEN, 61);
    assert_eq!(SNAPSHOT_HEADER_LEN, 77);
    assert_eq!(HEAD_RECORD_LEN, 113);
    assert_eq!(SIGNED_HEAD_LEN, 177);
    assert_eq!(PAIR_ROLE_OPENER, 0);
    assert_eq!(PAIR_ROLE_TYPIST, 1);
    assert_eq!(kb_types::sync::PROTOCOL_VERSION, 2);
}

// ----- golden layouts ------------------------------------------------------------------------

/// The expected bytes of `parts`, concatenated.
fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

#[test]
fn the_entry_header_has_the_documented_layout() {
    let bytes = encode_entry_header(&entry_header());
    assert_eq!(bytes.len(), ENTRY_HEADER_LEN);
    assert_eq!(
        bytes,
        cat(&[
            &[2],
            &[0xf1; 16],
            &[0xa1; 16],
            &[0xd1; 16],
            &[0xe1; 16],
            &[8, 7, 6, 5, 4, 3, 2, 1],
            &[0x0d, 0x0c, 0x0b, 0x0a],
        ])
    );
}

#[test]
fn the_keyring_header_has_the_documented_layout() {
    let bytes = encode_keyring_header(&keyring_header());
    assert_eq!(bytes.len(), KEYRING_HEADER_LEN);
    assert_eq!(
        bytes,
        cat(&[
            &[2],
            &[0xf2; 16],
            &[0xa2; 16],
            &[0xe2; 16],
            &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11],
            &[7, 0, 0, 0],
        ])
    );
}

#[test]
fn the_snapshot_header_has_the_documented_layout() {
    let bytes = encode_snapshot_header(&snapshot_header());
    assert_eq!(bytes.len(), SNAPSHOT_HEADER_LEN);
    assert_eq!(
        bytes,
        cat(&[
            &[2],
            &[0xf3; 16],
            &[0xa3; 16],
            &[0xd3; 16],
            &[0xe3; 16],
            &[0x28, 0x27, 0x26, 0x25, 0x24, 0x23, 0x22, 0x21],
            &[9, 0, 0, 0],
        ])
    );
}

#[test]
fn a_signed_head_has_the_documented_layout() {
    let signed = sign_head(&head(), &signer(1));
    assert_eq!(signed.len(), SIGNED_HEAD_LEN);
    let record = cat(&[
        &[2],
        &[0xf4; 16],
        &[0xa4; 16],
        &[0xe4; 16],
        &[0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31],
        &[0xc4; 32],
        &[0x5e; 16],
        &[8, 7, 6, 5, 4, 3, 2, 1],
    ]);
    assert_eq!(&signed[..HEAD_RECORD_LEN], record.as_slice());
    assert_eq!(signed.len() - HEAD_RECORD_LEN, SIGNATURE_LEN);
    assert_eq!(peek_head(&signed).unwrap(), head());
    assert_eq!(
        verify_head(&signed, &signer(1).verifying_key()).unwrap(),
        head()
    );
}

#[test]
fn a_sealed_entry_carries_its_header_in_the_clear() {
    let rng = SeededRng::new(1);
    let sealed = seal_entry(&entry_header(), &entry_body(), &key(2), &signer(3), &rng);
    let envelope = Envelope::decode(&sealed).unwrap();
    assert_eq!(envelope.kind, EnvelopeKind::Entry);
    assert_eq!(envelope.header, encode_entry_header(&entry_header()));
    assert!(envelope.signature.is_some());
}

// ----- round trips ---------------------------------------------------------------------------

#[test]
fn an_entry_round_trips_and_its_header_peeks_without_a_key() {
    let rng = SeededRng::new(2);
    let sealed = seal_entry(&entry_header(), &entry_body(), &key(2), &signer(3), &rng);
    assert_eq!(peek_entry(&sealed).unwrap(), entry_header());
    let (header, body) = open_entry(&sealed, &key(2), &signer(3).verifying_key()).unwrap();
    assert_eq!(header, entry_header());
    assert_eq!(body, entry_body());
    // Sealing twice differs (a fresh nonce), the header does not.
    let again = seal_entry(&entry_header(), &entry_body(), &key(2), &signer(3), &rng);
    assert_ne!(sealed, again);
}

#[test]
fn a_keyring_entry_round_trips() {
    let rng = SeededRng::new(3);
    let items = keyring_items(&rng);
    let sealed = seal_keyring(&keyring_header(), &items, &key(4), &signer(5), &rng);
    assert_eq!(peek_keyring(&sealed).unwrap(), keyring_header());
    let (header, opened) = open_keyring(&sealed, &key(4), &signer(5).verifying_key()).unwrap();
    assert_eq!(header, keyring_header());
    assert_eq!(opened, items);
}

#[test]
fn a_snapshot_round_trips() {
    let rng = SeededRng::new(4);
    let sealed = seal_snapshot(
        &snapshot_header(),
        &snapshot_body(),
        &key(6),
        &signer(7),
        &rng,
    );
    assert_eq!(peek_snapshot(&sealed).unwrap(), snapshot_header());
    let (header, body) = open_snapshot(&sealed, &key(6), &signer(7).verifying_key()).unwrap();
    assert_eq!(header, snapshot_header());
    assert_eq!(body, snapshot_body());
}

#[test]
fn a_membership_entry_peeks_to_its_header_and_verifies() {
    let alice = Person::generate(5, "Alice", FIXTURE_TIME_MS);
    let mut space = Space::new(6, "Codec", FIXTURE_TIME_MS);
    let create = space.create(&alice, 1);
    let sealed = create.encode();
    assert_eq!(peek_membership(&sealed).unwrap(), create.header);
    assert_eq!(feed_kind(&sealed).unwrap(), ItemKind::Membership);
    let public = alice.devices[0].public.signing;
    verify_signature(&sealed, &public).unwrap();
    assert_eq!(
        verify_signature(&sealed, &signer(1).verifying_key()),
        Err(SyncError::Authentication)
    );
    let info = peek_feed_item(&sealed).unwrap();
    assert_eq!(
        info,
        FeedItemInfo {
            kind: ItemKind::Membership,
            feed: space.feed,
            account_id: alice.account,
            doc_id: KEYRING_DOC_ID,
            device_id: alice.device_id(0),
            device_seq: 0,
        }
    );
    assert!(matches!(
        peek_membership(&sealed[..sealed.len() - 3]),
        Err(SyncError::Encoding(_))
    ));
}

// ----- tampering -----------------------------------------------------------------------------

#[test]
fn every_region_of_a_sealed_object_is_authenticated() {
    let rng = SeededRng::new(5);
    let sealed = seal_entry(&entry_header(), &entry_body(), &key(2), &signer(3), &rng);
    let verifier = signer(3).verifying_key();
    // Header, nonce, ciphertext and signature: any flipped byte fails authentication (the layout
    // is canonical, so flipping a length field makes it malformed instead).
    for index in [
        3 + 4 + 5,
        3 + 4 + ENTRY_HEADER_LEN + 3,
        sealed.len() / 2,
        sealed.len() - SIGNATURE_LEN - 5,
        sealed.len() - 4,
    ] {
        let mut copy = sealed.clone();
        copy[index] ^= 0x01;
        assert!(
            matches!(
                open_entry(&copy, &key(2), &verifier),
                Err(SyncError::Authentication)
            ),
            "byte {index}"
        );
        assert_eq!(
            verify_signature(&copy, &verifier),
            Err(SyncError::Authentication)
        );
    }
    assert_eq!(
        open_entry(&sealed, &key(9), &verifier),
        Err(SyncError::Authentication),
        "a wrong key"
    );
    assert_eq!(
        open_entry(&sealed, &key(2), &signer(4).verifying_key()),
        Err(SyncError::Authentication),
        "a wrong verifier"
    );
}

#[test]
fn a_tampered_head_fails_its_signature() {
    let signed = sign_head(&head(), &signer(1));
    for index in [1, 20, 40, HEAD_RECORD_LEN - 1, SIGNED_HEAD_LEN - 1] {
        let mut copy = signed.clone();
        copy[index] ^= 1;
        assert!(
            matches!(
                verify_head(&copy, &signer(1).verifying_key()),
                Err(SyncError::Authentication | SyncError::Encoding(_))
            ),
            "byte {index}"
        );
    }
    assert_eq!(
        verify_head(&signed, &signer(2).verifying_key()),
        Err(SyncError::Authentication)
    );
}

// ----- decoders ------------------------------------------------------------------------------

#[test]
fn a_v1_object_is_refused_with_protocol() {
    let rng = SeededRng::new(6);
    // Objects of the retired protocol: the headers of ADR-0021 (version byte 1, no feed), in
    // envelopes signed and sealed like any other.
    let ids = |parts: &[&[u8]]| parts.concat();
    let account = [0xa1; 16];
    let doc = [0xd1; 16];
    let device = [0xe1; 16];
    let v1_entry_header = ids(&[
        &[1],
        &account,
        &doc,
        &device,
        &1u64.to_le_bytes(),
        &0u32.to_le_bytes(),
    ]);
    let v1_entry = seal_raw(EnvelopeKind::Entry, &v1_entry_header, b"x", &rng);
    assert!(matches!(peek_entry(&v1_entry), Err(SyncError::Protocol(_))));
    assert!(matches!(
        open_entry(&v1_entry, &key(1), &signer(2).verifying_key()),
        Err(SyncError::Protocol(_))
    ));
    assert!(matches!(
        peek_feed_item(&v1_entry),
        Err(SyncError::Protocol(_))
    ));
    let v1_snapshot_header = ids(&[
        &[1],
        &account,
        &doc,
        &device,
        &4u64.to_le_bytes(),
        &0u32.to_le_bytes(),
    ]);
    let v1_snapshot = seal_raw(EnvelopeKind::Snapshot, &v1_snapshot_header, b"x", &rng);
    assert!(matches!(
        peek_snapshot(&v1_snapshot),
        Err(SyncError::Protocol(_))
    ));
    let v1_keyring_header = ids(&[&[1], &account, &device, &1u64.to_le_bytes()]);
    let v1_keyring = seal_raw(EnvelopeKind::Keyring, &v1_keyring_header, b"x", &rng);
    assert!(matches!(
        peek_keyring(&v1_keyring),
        Err(SyncError::Protocol(_))
    ));
    let mut v1_head = ids(&[
        &[1],
        &account,
        &device,
        &3u64.to_le_bytes(),
        &[1; 32],
        &[2; 16],
        &4i64.to_le_bytes(),
    ]);
    let signature = kb_crypto::sign(&signer(3), HEAD_CONTEXT, &v1_head);
    v1_head.extend_from_slice(&signature.to_bytes());
    assert!(matches!(peek_head(&v1_head), Err(SyncError::Protocol(_))));
}

#[test]
fn malformed_headers_are_encoding_errors() {
    let rng = SeededRng::new(7);
    let sealed = seal_entry(&entry_header(), &entry_body(), &key(2), &signer(3), &rng);
    let mut envelope = Envelope::decode(&sealed).unwrap();
    for header in [
        encode_entry_header(&entry_header())[..40].to_vec(),
        [encode_entry_header(&entry_header()), vec![0]].concat(),
        Vec::new(),
        {
            let mut other = encode_entry_header(&entry_header());
            other[0] = 9;
            other
        },
    ] {
        envelope.header = header;
        assert!(
            matches!(peek_entry(&envelope.encode()), Err(SyncError::Encoding(_))),
            "a malformed header"
        );
    }
    // A keyring envelope is not an entry, whatever its header.
    let keyring = seal_keyring(&keyring_header(), &[], &key(2), &signer(3), &rng);
    assert!(matches!(peek_entry(&keyring), Err(SyncError::Encoding(_))));
    assert!(matches!(
        peek_entry(b"nonsense"),
        Err(SyncError::Encoding(_))
    ));
    assert!(matches!(peek_head(&[2; 10]), Err(SyncError::Encoding(_))));
    let mut long = sign_head(&head(), &signer(1));
    long.push(0);
    assert!(matches!(peek_head(&long), Err(SyncError::Encoding(_))));
}

#[test]
fn feed_items_are_told_apart_by_their_envelope_kind() {
    let rng = SeededRng::new(8);
    let entry = seal_entry(&entry_header(), &entry_body(), &key(2), &signer(3), &rng);
    let keyring = seal_keyring(&keyring_header(), &[], &key(2), &signer(3), &rng);
    let snapshot = seal_snapshot(
        &snapshot_header(),
        &snapshot_body(),
        &key(2),
        &signer(3),
        &rng,
    );
    assert_eq!(feed_kind(&entry).unwrap(), ItemKind::Entry);
    assert_eq!(feed_kind(&keyring).unwrap(), ItemKind::Keyring);
    assert!(matches!(feed_kind(&snapshot), Err(SyncError::Encoding(_))));
    assert!(matches!(feed_kind(&[]), Err(SyncError::Encoding(_))));
    assert!(matches!(feed_kind(&[9, 1, 1]), Err(SyncError::Encoding(_))));
    assert!(matches!(feed_kind(&[1, 9, 1]), Err(SyncError::Encoding(_))));
    assert!(matches!(
        feed_kind(&[1, 1, 99]),
        Err(SyncError::Encoding(_))
    ));
    assert_eq!(
        peek_feed_item(&entry).unwrap(),
        FeedItemInfo {
            kind: ItemKind::Entry,
            feed: entry_header().feed,
            account_id: entry_header().account_id,
            doc_id: entry_header().doc_id,
            device_id: entry_header().device_id,
            device_seq: entry_header().device_seq,
        }
    );
    let info = peek_feed_item(&keyring).unwrap();
    assert_eq!(info.kind, ItemKind::Keyring);
    assert_eq!(info.doc_id, KEYRING_DOC_ID);
    assert_eq!(info.feed, keyring_header().feed);
    assert_eq!(info.device_seq, keyring_header().device_seq);
    // Verification of the signature works on every kind, keys unknown.
    let verifier = signer(3).verifying_key();
    for sealed in [&entry, &keyring, &snapshot] {
        verify_signature(sealed, &verifier).unwrap();
    }
}

#[test]
fn the_chain_helpers_are_blake3_of_the_previous_value_and_the_bytes() {
    let epoch = [7u8; 16];
    assert_eq!(chain_genesis(&epoch), kb_crypto::hash(&epoch));
    let genesis = chain_genesis(&epoch);
    let mut joined = genesis.to_vec();
    joined.extend_from_slice(b"item");
    assert_eq!(chain_next(&genesis, b"item"), kb_crypto::hash(&joined));
    assert_eq!(
        chain_next(&genesis, b"item"),
        crate::chain_next(&genesis, b"item")
    );
    assert_ne!(
        chain_next(&genesis, b"item"),
        chain_next(&genesis, b"other")
    );
    assert_ne!(chain_genesis(&[8u8; 16]), genesis);
}

// ----- bodies behind a valid signature (shared with the crate root) ----------------------------

fn is_encoding<T: core::fmt::Debug>(result: Result<T, SyncError>) -> bool {
    matches!(result, Err(SyncError::Encoding(_)))
}

fn sealed_entry(rng: &SeededRng) -> Vec<u8> {
    seal_entry(&entry_header(), &entry_body(), &key(1), &signer(2), rng)
}

/// Seals `plaintext` (padded) as a signed envelope of `kind` with `header`, so the decoders behind
/// a valid signature and tag can be exercised.
fn seal_raw(kind: EnvelopeKind, header: &[u8], plaintext: &[u8], rng: &SeededRng) -> Vec<u8> {
    let padded = kb_crypto::pad_to_bucket(plaintext, rng);
    kb_crypto::seal_envelope(kind, header, &key(1), &padded, Some(&signer(2)), rng)
}

#[test]
fn entry_body_plaintext_is_the_documented_layout() {
    let rng = SeededRng::new(2);
    let body = EntryBody {
        engine: 9,
        format_version: 0x0102,
        min_reader_version: 0x0304,
        doc_kind: 5,
        updates: vec![b"ab".to_vec(), b"c".to_vec()],
        version_vector: b"v".to_vec(),
    };
    let bytes = seal_entry(&entry_header(), &body, &key(1), &signer(2), &rng);
    let envelope = Envelope::decode(&bytes).unwrap();
    let padded = kb_crypto::aead_open(
        &key(1),
        &envelope.nonce,
        &envelope.aad(),
        &envelope.ciphertext,
    )
    .unwrap();
    let plaintext = kb_crypto::unpad(&padded).unwrap();
    let expected: &[u8] = &[
        9, 0x02, 0x01, 0x04, 0x03, 5, 2, 0, 0, 0, 2, 0, 0, 0, b'a', b'b', 1, 0, 0, 0, b'c', 1, 0,
        0, 0, b'v',
    ];
    assert_eq!(plaintext, expected);
    // The body is padded to a bucket before encryption.
    let padded_len = envelope.ciphertext.len() - kb_crypto::TAG_LEN;
    assert_eq!(kb_crypto::bucket_size(padded_len), padded_len);
}

#[test]
fn an_entry_without_updates_round_trips() {
    let rng = SeededRng::new(3);
    let body = EntryBody {
        updates: Vec::new(),
        version_vector: Vec::new(),
        ..entry_body()
    };
    let bytes = seal_entry(&entry_header(), &body, &key(1), &signer(2), &rng);
    let (_, opened) = open_entry(&bytes, &key(1), &signer(2).verifying_key()).unwrap();
    assert_eq!(opened, body);
}

#[test]
fn wrong_key_wrong_signer_and_missing_signature_are_authentication() {
    let rng = SeededRng::new(6);
    let bytes = sealed_entry(&rng);
    let verifier = signer(2).verifying_key();
    assert_eq!(
        open_entry(&bytes, &key(9), &verifier),
        Err(SyncError::Authentication)
    );
    assert_eq!(
        open_entry(&bytes, &key(1), &signer(3).verifying_key()),
        Err(SyncError::Authentication)
    );
    let unsigned = &bytes[..bytes.len() - SIGNATURE_LEN];
    assert_eq!(peek_entry(unsigned).unwrap(), entry_header());
    assert_eq!(
        open_entry(unsigned, &key(1), &verifier),
        Err(SyncError::Authentication)
    );
    assert_eq!(
        verify_signature(unsigned, &verifier),
        Err(SyncError::Authentication)
    );
    verify_signature(&bytes, &verifier).unwrap();
    // The device signature is the envelope signature of kind Entry.
    let envelope = Envelope::decode(&bytes).unwrap();
    kb_crypto::verify(
        &verifier,
        "envelope/entry",
        &bytes[..bytes.len() - SIGNATURE_LEN],
        envelope.signature.as_ref().unwrap(),
    )
    .unwrap();
}

#[test]
fn an_entry_under_another_kind_byte_is_encoding() {
    let rng = SeededRng::new(5);
    let bytes = sealed_entry(&rng);
    let verifier = signer(2).verifying_key();
    for kind in [
        EnvelopeKind::Keyring,
        EnvelopeKind::Snapshot,
        EnvelopeKind::Head,
    ] {
        let mut changed = bytes.clone();
        changed[2] = kind.to_byte();
        assert!(is_encoding(peek_entry(&changed)), "{kind:?}");
        assert!(
            is_encoding(open_entry(&changed, &key(1), &verifier)),
            "{kind:?}"
        );
    }
    assert!(is_encoding(peek_snapshot(&bytes)));
    assert!(is_encoding(peek_keyring(&bytes)));
    assert!(is_encoding(open_snapshot(&bytes, &key(1), &verifier)));
}

#[test]
fn truncated_or_extended_entries_are_encoding_or_authentication() {
    let rng = SeededRng::new(7);
    let bytes = sealed_entry(&rng);
    let verifier = signer(2).verifying_key();
    for len in 0..bytes.len() {
        let result = open_entry(&bytes[..len], &key(1), &verifier);
        if len == bytes.len() - SIGNATURE_LEN {
            assert_eq!(result, Err(SyncError::Authentication));
        } else {
            assert!(is_encoding(result), "prefix of {len}");
            assert!(feed_kind(&bytes[..len]).is_err() || len >= 3);
        }
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(is_encoding(peek_entry(&longer)));
}

#[test]
fn malformed_bodies_behind_a_valid_signature_are_encoding() {
    let rng = SeededRng::new(8);
    let verifier = signer(2).verifying_key();
    let header = Envelope::decode(&sealed_entry(&rng)).unwrap().header;
    let good: Vec<u8> = vec![1, 1, 0, 1, 0, 2, 1, 0, 0, 0, 1, 0, 0, 0, b'u', 0, 0, 0, 0];
    let ok = seal_raw(EnvelopeKind::Entry, &header, &good, &rng);
    assert!(open_entry(&ok, &key(1), &verifier).is_ok());
    let mut trailing = good.clone();
    trailing.push(0);
    let mut huge_count = good.clone();
    huge_count[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut long_blob = good.clone();
    long_blob[10..14].copy_from_slice(&1000u32.to_le_bytes());
    for (name, body) in [
        ("trailing byte", trailing),
        ("count beyond the input", huge_count),
        ("blob beyond the input", long_blob),
        ("truncated prefix", vec![1, 1]),
        ("empty", Vec::new()),
    ] {
        let sealed = seal_raw(EnvelopeKind::Entry, &header, &body, &rng);
        assert!(
            is_encoding(open_entry(&sealed, &key(1), &verifier)),
            "{name}"
        );
    }
    // Ciphertext that is not a padding bucket.
    let unpadded = kb_crypto::seal_envelope(
        EnvelopeKind::Entry,
        &header,
        &key(1),
        &good,
        Some(&signer(2)),
        &rng,
    );
    assert!(is_encoding(open_entry(&unpadded, &key(1), &verifier)));
}

#[test]
fn keyring_bodies_must_hold_decodable_items_and_the_key_is_the_one_kb_keys_seals_under() {
    let rng = SeededRng::new(11);
    let header = encode_keyring_header(&keyring_header());
    let verifier = signer(2).verifying_key();
    let empty = seal_raw(EnvelopeKind::Keyring, &header, &[0, 0, 0, 0], &rng);
    assert_eq!(
        open_keyring(&empty, &key(1), &verifier).unwrap().1,
        Vec::new()
    );
    let junk_item = seal_raw(
        EnvelopeKind::Keyring,
        &header,
        &[1, 0, 0, 0, 3, 0, 0, 0, 9, 9, 9],
        &rng,
    );
    assert!(is_encoding(open_keyring(&junk_item, &key(1), &verifier)));
    // The keyring key is HKDF(ARK, "keyring"), the key kb-keys seals items under.
    let ark = Ark::from_secret(key(0x77));
    let items = keyring_items(&rng);
    let sealed_item = kb_keys::keyring::seal_item(&items[0], &ark, &rng);
    let (_, plain) =
        kb_crypto::open_envelope(&sealed_item, &crate::keyring_key(&ark), None).unwrap();
    assert_eq!(plain.as_slice(), items[0].encode());
}

#[test]
fn errors_never_carry_content() {
    let rng = SeededRng::new(14);
    let mut bytes = sealed_entry(&rng);
    bytes.truncate(40);
    let message = peek_entry(&bytes).unwrap_err().to_string();
    assert!(!message.contains("first update"));
    assert!(message.len() < 120, "{message}");
}
