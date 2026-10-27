//! Keyring: encodings, signatures, key operations, merge rule, transport sealing.

#![allow(clippy::unwrap_used)]

use std::sync::OnceLock;

use super::*;
use crate::test_support::{ark, device};
use kb_crypto::Envelope;
use kb_types::testing::SeededRng;
use proptest::prelude::*;

const T0: i64 = 1_760_000_000_000;

type Device = (DeviceId, DeviceKeys);

fn space(n: u8) -> SpaceId {
    SpaceId::from_bytes([n; 16])
}

fn doc(n: u8) -> DocId {
    DocId::from_bytes([n; 16])
}

fn attachment(n: u8) -> AttachmentId {
    AttachmentId::from_bytes([n; 16])
}

fn link(n: u8) -> LinkId {
    LinkId::from_bytes([n; 16])
}

fn space_record(
    dev: &Device,
    space_id: SpaceId,
    epoch: u32,
    at: i64,
    seed: u64,
) -> (Secret32, KeyRecord) {
    new_space_key(
        &ark(1),
        space_id,
        epoch,
        &dev.1,
        dev.0,
        at,
        &SeededRng::new(seed),
    )
}

fn auth_error<T: core::fmt::Debug>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Crypto(CryptoError::Authentication)))
}

fn corrupted<T: core::fmt::Debug>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Corrupted(_)))
}

// ---------------------------------------------------------------------------------
// Encodings

#[test]
fn subject_encoding_is_frozen() {
    let cases = [
        (
            KeySubject::Space { epoch: 0x0102_0304 },
            vec![1, 4, 3, 2, 1],
        ),
        (
            KeySubject::Doc {
                doc_id: doc(7),
                epoch: 2,
            },
            [vec![2], vec![7; 16], vec![2, 0, 0, 0]].concat(),
        ),
        (
            KeySubject::Attachment {
                attachment_id: attachment(9),
            },
            [vec![3], vec![9; 16]].concat(),
        ),
        (
            KeySubject::Link { link_id: link(8) },
            [vec![4], vec![8; 16]].concat(),
        ),
    ];
    for (subject, bytes) in cases {
        assert_eq!(subject.encode(), bytes, "{subject:?}");
        assert_eq!(KeySubject::decode(&bytes).unwrap(), subject);
        for len in 0..bytes.len() {
            assert!(corrupted(KeySubject::decode(&bytes[..len])), "{len}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(corrupted(KeySubject::decode(&longer)));
    }
    for tag in [0u8, 5, 0xFF] {
        assert!(corrupted(KeySubject::decode(&[tag, 0, 0, 0, 0])), "{tag}");
    }
}

#[test]
fn record_encoding_is_the_documented_one() {
    let dev = device(1);
    let (_, record) = space_record(&dev, space(5), 3, T0, 1);
    let item = KeyringItem::Record(record.clone());

    let mut expected = vec![1u8];
    expected.extend_from_slice(&[5; 16]);
    expected.extend_from_slice(&[1, 3, 0, 0, 0]);
    expected.extend_from_slice(&record.wrapped_key.to_bytes());
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(&record.kcv);
    expected.extend_from_slice(dev.0.as_bytes());
    expected.extend_from_slice(&T0.to_le_bytes());
    let signed = expected.clone();
    expected.extend_from_slice(&record.signature.to_bytes());

    let bytes = item.encode();
    assert_eq!(bytes, expected);
    assert_eq!(bytes.len(), 1 + 16 + 5 + 72 + 4 + 8 + 16 + 8 + 64);
    assert_eq!(KeyringItem::decode(&bytes).unwrap(), item);
    // The signature covers everything before it, with the keyring context.
    kb_crypto::verify(
        &dev.1.public().signing,
        KEYRING_CONTEXT,
        &signed,
        &record.signature,
    )
    .unwrap();
}

#[test]
fn tombstone_encoding_is_the_documented_one() {
    let dev = device(1);
    let subject = KeySubject::Doc {
        doc_id: doc(7),
        epoch: 1,
    };
    let t = tombstone(space(5), subject, &dev.1, dev.0, T0);
    assert_eq!(t.purged_by, dev.0);
    assert_eq!(t.purged_at, T0);
    let item = KeyringItem::Tombstone(t.clone());

    let mut expected = vec![2u8];
    expected.extend_from_slice(&[5; 16]);
    expected.extend_from_slice(&subject.encode());
    expected.extend_from_slice(dev.0.as_bytes());
    expected.extend_from_slice(&T0.to_le_bytes());
    let signed = expected.clone();
    expected.extend_from_slice(&t.signature.to_bytes());

    let bytes = item.encode();
    assert_eq!(bytes, expected);
    assert_eq!(bytes.len(), 1 + 16 + 21 + 16 + 8 + 64);
    assert_eq!(KeyringItem::decode(&bytes).unwrap(), item);
    kb_crypto::verify(
        &dev.1.public().signing,
        KEYRING_CONTEXT,
        &signed,
        &t.signature,
    )
    .unwrap();
}

#[test]
fn item_decode_rejects_malformed_bytes() {
    let dev = device(1);
    let (_, record) = space_record(&dev, space(5), 0, T0, 1);
    let items = [
        KeyringItem::Record(record),
        KeyringItem::Tombstone(tombstone(
            space(5),
            KeySubject::Attachment {
                attachment_id: attachment(2),
            },
            &dev.1,
            dev.0,
            T0,
        )),
    ];
    for item in items {
        let bytes = item.encode();
        for len in 0..bytes.len() {
            assert!(corrupted(KeyringItem::decode(&bytes[..len])), "{len}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(corrupted(KeyringItem::decode(&longer)));
        for tag in [0u8, 3, 0xFF] {
            let mut other = bytes.clone();
            other[0] = tag;
            assert!(corrupted(KeyringItem::decode(&other)), "item tag {tag}");
        }
        // The subject tag sits right after the space id.
        let mut other = bytes.clone();
        other[17] = 9;
        assert!(corrupted(KeyringItem::decode(&other)));
    }
}

// ---------------------------------------------------------------------------------
// Signatures

#[test]
fn items_verify_against_their_device_only() {
    let (a, b) = (device(1), device(2));
    let (_, record) = space_record(&a, space(5), 0, T0, 1);
    let t = tombstone(space(5), KeySubject::Space { epoch: 0 }, &b.1, b.0, T0);

    KeyringItem::Record(record.clone())
        .verify(&a.1.public().signing)
        .unwrap();
    KeyringItem::Tombstone(t.clone())
        .verify(&b.1.public().signing)
        .unwrap();
    assert!(auth_error(
        KeyringItem::Record(record.clone()).verify(&b.1.public().signing)
    ));
    assert!(auth_error(
        KeyringItem::Tombstone(t.clone()).verify(&a.1.public().signing)
    ));

    let tampered_records = [
        KeyRecord {
            space_id: space(6),
            ..record.clone()
        },
        KeyRecord {
            subject: KeySubject::Space { epoch: 1 },
            ..record.clone()
        },
        KeyRecord {
            wrap_epoch: 1,
            ..record.clone()
        },
        KeyRecord {
            kcv: [0; 8],
            ..record.clone()
        },
        KeyRecord {
            created_by: b.0,
            ..record.clone()
        },
        KeyRecord {
            created_at: T0 + 1,
            ..record.clone()
        },
    ];
    for tampered in tampered_records {
        assert!(auth_error(
            KeyringItem::Record(tampered).verify(&a.1.public().signing)
        ));
    }
    let moved = Tombstone {
        subject: KeySubject::Space { epoch: 1 },
        ..t.clone()
    };
    assert!(auth_error(
        KeyringItem::Tombstone(moved).verify(&b.1.public().signing)
    ));

    // A record and a tombstone never share a signed message (the tag byte differs).
    let signed_in_log_context = KeyRecord {
        signature: kb_crypto::sign(a.1.signing(), "device-log", &record.signed_bytes()),
        ..record
    };
    assert!(auth_error(
        KeyringItem::Record(signed_in_log_context).verify(&a.1.public().signing)
    ));
}

// ---------------------------------------------------------------------------------
// Key operations

#[test]
fn space_kek_is_hkdf_of_the_ark_per_space() {
    let root = ark(1);
    assert_eq!(
        space_kek(&root, space(5)),
        kb_crypto::derive_key(root.secret(), Label::SPACE_KEK, &[5; 16])
    );
    assert_ne!(space_kek(&root, space(5)), space_kek(&root, space(6)));
    assert_ne!(space_kek(&root, space(5)), space_kek(&ark(2), space(5)));
}

#[test]
fn space_keys_are_wrapped_under_the_space_kek() {
    let dev = device(1);
    let root = ark(1);
    let (key, record) = new_space_key(&root, space(5), 3, &dev.1, dev.0, T0, &SeededRng::new(1));

    assert_eq!(record.space_id, space(5));
    assert_eq!(record.subject, KeySubject::Space { epoch: 3 });
    assert_eq!(record.wrap_epoch, 0, "first ARK generation");
    assert_eq!(record.kcv, kb_crypto::key_check_value(&key));
    assert_eq!(record.created_by, dev.0);
    assert_eq!(record.created_at, T0);
    KeyringItem::Record(record.clone())
        .verify(&dev.1.public().signing)
        .unwrap();

    assert_eq!(unwrap_space_key(&root, &record).unwrap(), key);
    // The documented construction.
    let subject = [&[5u8; 16][..], &KeySubject::Space { epoch: 3 }.encode()].concat();
    assert_eq!(
        kb_crypto::unwrap_key(&record.wrapped_key, &space_kek(&root, space(5)), &subject).unwrap(),
        key
    );

    let (other_key, other_record) =
        new_space_key(&root, space(5), 3, &dev.1, dev.0, T0, &SeededRng::new(2));
    assert_ne!(other_key, key, "a fresh random key each time");
    assert_ne!(other_record.wrapped_key, record.wrapped_key);
}

#[test]
fn a_space_key_that_exists_is_wrapped_like_a_fresh_one_for_the_personal_keyring() {
    let dev = device(1);
    let root = ark(1);
    // A key opened from a membership box (a shared space's epoch) stored as a `Space` record.
    let key = Secret32::generate(&SeededRng::new(9));
    let record = wrap_space_key(
        &key,
        &root,
        space(5),
        4,
        2,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(1),
    );
    assert_eq!(record.space_id, space(5));
    assert_eq!(record.subject, KeySubject::Space { epoch: 4 });
    assert_eq!(record.wrap_epoch, 2);
    assert_eq!(record.kcv, kb_crypto::key_check_value(&key));
    KeyringItem::Record(record.clone())
        .verify(&dev.1.public().signing)
        .unwrap();
    assert_eq!(unwrap_space_key(&root, &record).unwrap(), key);
    assert!(unwrap_space_key(&ark(2), &record).is_err());
    // It merges and re-wraps like any space record.
    let mut keyring = Keyring::new();
    assert!(keyring.merge(KeyringItem::Record(record.clone())));
    let rewrapped = rewrap_space_record(
        &record,
        &root,
        &ark(2),
        &dev.1,
        dev.0,
        T0 + 1,
        &SeededRng::new(3),
    )
    .unwrap();
    assert_eq!(rewrapped.wrap_epoch, 3);
    assert_eq!(unwrap_space_key(&ark(2), &rewrapped).unwrap(), key);
}

#[test]
fn space_keys_need_the_right_ark_space_and_subject() {
    let dev = device(1);
    let root = ark(1);
    let (_, record) = new_space_key(&root, space(5), 3, &dev.1, dev.0, T0, &SeededRng::new(1));

    assert!(auth_error(unwrap_space_key(&ark(2), &record)));
    assert!(auth_error(unwrap_space_key(
        &root,
        &KeyRecord {
            space_id: space(6),
            ..record.clone()
        }
    )));
    assert!(auth_error(unwrap_space_key(
        &root,
        &KeyRecord {
            subject: KeySubject::Space { epoch: 4 },
            ..record.clone()
        }
    )));
    // Intact ciphertext, wrong check value: still refused.
    assert!(auth_error(unwrap_space_key(
        &root,
        &KeyRecord {
            kcv: [0; 8],
            ..record.clone()
        }
    )));
    let mut wrapped = record.wrapped_key.to_bytes();
    wrapped[40] ^= 1;
    assert!(auth_error(unwrap_space_key(
        &root,
        &KeyRecord {
            wrapped_key: WrappedKey::from_bytes(&wrapped).unwrap(),
            ..record.clone()
        }
    )));

    let (space_key, _) = new_space_key(&root, space(5), 0, &dev.1, dev.0, T0, &SeededRng::new(3));
    let (_, doc_record) = new_doc_key(
        &space_key,
        space(5),
        doc(1),
        0,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(4),
    );
    let err = unwrap_space_key(&root, &doc_record).unwrap_err();
    assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
}

#[test]
fn doc_keys_are_wrapped_under_the_space_key() {
    let dev = device(1);
    let (space_key, _) = space_record(&dev, space(5), 2, T0, 1);
    let (key, record) = new_doc_key(
        &space_key,
        space(5),
        doc(8),
        2,
        &dev.1,
        dev.0,
        T0 + 1,
        &SeededRng::new(2),
    );

    assert_eq!(
        record.subject,
        KeySubject::Doc {
            doc_id: doc(8),
            epoch: 2
        }
    );
    assert_eq!(record.wrap_epoch, 2);
    assert_eq!(record.kcv, kb_crypto::key_check_value(&key));
    assert_eq!((record.created_by, record.created_at), (dev.0, T0 + 1));
    KeyringItem::Record(record.clone())
        .verify(&dev.1.public().signing)
        .unwrap();
    assert_eq!(unwrap_doc_key(&space_key, &record).unwrap(), key);
    let subject = [&[5u8; 16][..], &record.subject.encode()].concat();
    assert_eq!(
        kb_crypto::unwrap_key(&record.wrapped_key, &space_key, &subject).unwrap(),
        key
    );

    let (other_space_key, _) = space_record(&dev, space(5), 2, T0, 3);
    assert!(auth_error(unwrap_doc_key(&other_space_key, &record)));
    for moved in [
        KeyRecord {
            space_id: space(6),
            ..record.clone()
        },
        KeyRecord {
            subject: KeySubject::Doc {
                doc_id: doc(9),
                epoch: 2,
            },
            ..record.clone()
        },
        KeyRecord {
            subject: KeySubject::Doc {
                doc_id: doc(8),
                epoch: 3,
            },
            ..record.clone()
        },
        KeyRecord {
            kcv: [1; 8],
            ..record.clone()
        },
    ] {
        assert!(auth_error(unwrap_doc_key(&space_key, &moved)));
    }
    assert!(corrupted(unwrap_attachment_key(&space_key, &record)));
}

#[test]
fn a_view_link_key_is_wrapped_under_the_space_key_and_names_its_link() {
    let dev = device(1);
    let (space_key, _) = space_record(&dev, space(5), 1, T0, 1);
    let link_key = LinkKey::generate(&SeededRng::new(40));
    let record = wrap_link_key(
        &link_key,
        &space_key,
        space(5),
        1,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(2),
    );

    assert_eq!(
        record.subject,
        KeySubject::Link {
            link_id: link_key.link_id()
        }
    );
    assert_eq!(record.wrap_epoch, 1);
    assert_eq!(record.kcv, kb_crypto::key_check_value(link_key.secret()));
    KeyringItem::Record(record.clone())
        .verify(&dev.1.public().signing)
        .unwrap();
    // The item survives its encoding like any other.
    let item = KeyringItem::Record(record.clone());
    assert_eq!(KeyringItem::decode(&item.encode()).unwrap(), item);
    assert_eq!(unwrap_link_key(&space_key, &record).unwrap(), link_key);

    // A wrong space key, another space, another link's id or another kind of key do not open it.
    let (other_space_key, _) = space_record(&dev, space(5), 1, T0, 3);
    assert!(auth_error(unwrap_link_key(&other_space_key, &record)));
    assert!(auth_error(unwrap_link_key(
        &space_key,
        &KeyRecord {
            space_id: space(6),
            ..record.clone()
        }
    )));
    assert!(auth_error(unwrap_link_key(
        &space_key,
        &KeyRecord {
            subject: KeySubject::Link { link_id: link(5) },
            ..record.clone()
        }
    )));
    assert!(corrupted(unwrap_doc_key(&space_key, &record)));
    assert!(corrupted(unwrap_attachment_key(&space_key, &record)));
    let (_, doc_record) = new_doc_key(
        &space_key,
        space(5),
        doc(1),
        1,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(5),
    );
    assert!(corrupted(unwrap_link_key(&space_key, &doc_record)));

    // A record whose wrapped key does not derive the id it is filed under is refused: it was wrapped
    // for another link's id (a hostile or buggy writer), with a valid wrap of that subject.
    let liar = LinkKey::generate(&SeededRng::new(41));
    let forged = KeyRecord {
        subject: KeySubject::Link { link_id: link(9) },
        wrapped_key: kb_crypto::wrap_key(
            liar.secret(),
            &space_key,
            &wrap_subject(space(5), &KeySubject::Link { link_id: link(9) }),
            &SeededRng::new(6),
        ),
        kcv: kb_crypto::key_check_value(liar.secret()),
        ..record
    };
    assert!(corrupted(unwrap_link_key(&space_key, &forged)));
}

#[test]
fn view_link_records_count_merge_and_tombstone_like_the_other_subjects() {
    let dev = device(1);
    let (space_key, _) = space_record(&dev, space(5), 1, T0, 1);
    let key = LinkKey::generate(&SeededRng::new(42));
    let record = wrap_link_key(
        &key,
        &space_key,
        space(5),
        1,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(2),
    );
    let subject = record.subject;
    let mut keyring = Keyring::new();
    assert!(keyring.merge(KeyringItem::Record(record.clone())));
    assert_eq!(keyring.counts().links, 1);
    assert_eq!(keyring.record(space(5), &subject), Some(&record));
    // A link record is not a document or an attachment: the holders index ignores it.
    assert!(
        keyring
            .doc_spaces(DocId::from_bytes(*key.link_id().as_bytes()))
            .is_empty()
    );

    // Shared feeds: the first record wins, and only a tombstone that counts removes it.
    let mut shared = Keyring::new();
    assert!(shared.merge_ordered(KeyringItem::Record(record.clone()), &|_| true));
    assert!(!shared.merge_ordered(KeyringItem::Record(record.clone()), &|_| true));
    let gone = tombstone(space(5), subject, &dev.1, dev.0, T0 + 1);
    assert!(!shared.merge_ordered(KeyringItem::Tombstone(gone.clone()), &|_| false));
    assert!(shared.record(space(5), &subject).is_some());
    assert!(shared.merge_ordered(KeyringItem::Tombstone(gone), &|_| true));
    assert!(shared.is_tombstoned(space(5), &subject));
    assert!(shared.record(space(5), &subject).is_none());
    assert!(!shared.merge_ordered(KeyringItem::Record(record), &|_| true));
    assert_eq!(shared.counts().tombstones, 1);
}

#[test]
fn attachment_keys_are_wrapped_under_the_space_key() {
    let dev = device(1);
    let (space_key, _) = space_record(&dev, space(5), 1, T0, 1);
    let (key, record) = new_attachment_key(
        &space_key,
        space(5),
        attachment(4),
        1,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(2),
    );

    assert_eq!(
        record.subject,
        KeySubject::Attachment {
            attachment_id: attachment(4)
        }
    );
    assert_eq!(record.wrap_epoch, 1);
    assert_eq!(record.kcv, kb_crypto::key_check_value(&key));
    KeyringItem::Record(record.clone())
        .verify(&dev.1.public().signing)
        .unwrap();
    assert_eq!(unwrap_attachment_key(&space_key, &record).unwrap(), key);

    let (other_space_key, _) = space_record(&dev, space(5), 1, T0, 3);
    assert!(auth_error(unwrap_attachment_key(&other_space_key, &record)));
    assert!(auth_error(unwrap_attachment_key(
        &space_key,
        &KeyRecord {
            subject: KeySubject::Attachment {
                attachment_id: attachment(5)
            },
            ..record.clone()
        }
    )));
    assert!(auth_error(unwrap_attachment_key(
        &space_key,
        &KeyRecord {
            space_id: space(6),
            ..record.clone()
        }
    )));
    assert!(corrupted(unwrap_doc_key(&space_key, &record)));
}

#[test]
fn an_attachment_key_can_be_wrapped_again_under_another_space() {
    let dev = device(1);
    let (key_a, _) = space_record(&dev, space(5), 0, T0, 1);
    let (key_b, _) = space_record(&dev, space(6), 0, T0, 2);
    let (key, record_a) = new_attachment_key(
        &key_a,
        space(5),
        attachment(4),
        0,
        &dev.1,
        dev.0,
        T0,
        &SeededRng::new(3),
    );
    let record_b = wrap_attachment_key(
        &key,
        &key_b,
        space(6),
        attachment(4),
        0,
        &dev.1,
        dev.0,
        T0 + 5,
        &SeededRng::new(4),
    );

    assert_eq!(record_b.space_id, space(6));
    assert_eq!(record_b.subject, record_a.subject);
    assert_eq!(record_b.kcv, record_a.kcv, "the same key");
    assert_eq!((record_b.created_by, record_b.created_at), (dev.0, T0 + 5));
    KeyringItem::Record(record_b.clone())
        .verify(&dev.1.public().signing)
        .unwrap();
    assert_eq!(unwrap_attachment_key(&key_b, &record_b).unwrap(), key);
    assert_eq!(unwrap_attachment_key(&key_a, &record_a).unwrap(), key);
    // Each record opens only under its own space's key.
    assert!(auth_error(unwrap_attachment_key(&key_a, &record_b)));
    assert!(auth_error(unwrap_attachment_key(&key_b, &record_a)));
}

#[test]
fn documents_and_attachments_are_found_in_every_space_that_holds_a_record() {
    let a = device(1);
    let (sk5, _) = space_record(&a, space(5), 0, T0, 1);
    let (sk6, _) = space_record(&a, space(6), 0, T0, 2);
    let doc_in = |sk: &Secret32, space_id: SpaceId, doc_n: u8, epoch: u32, seed: u64| {
        new_doc_key(
            sk,
            space_id,
            doc(doc_n),
            epoch,
            &a.1,
            a.0,
            T0,
            &SeededRng::new(seed),
        )
        .1
    };
    let d5 = doc_in(&sk5, space(5), 1, 0, 3);
    let d5_next = doc_in(&sk5, space(5), 1, 1, 4);
    let d6 = doc_in(&sk6, space(6), 1, 0, 5);
    let other = doc_in(&sk5, space(5), 2, 0, 6);
    let (_, att5) = new_attachment_key(
        &sk5,
        space(5),
        attachment(1),
        0,
        &a.1,
        a.0,
        T0,
        &SeededRng::new(7),
    );
    let att6 = wrap_attachment_key(
        &Secret32::generate(&SeededRng::new(8)),
        &sk6,
        space(6),
        attachment(1),
        0,
        &a.1,
        a.0,
        T0,
        &SeededRng::new(9),
    );

    let mut keyring = Keyring::new();
    assert!(keyring.doc_spaces(doc(1)).is_empty());
    for record in [&d6, &other, &d5_next, &att6, &d5, &att5] {
        keyring.merge(KeyringItem::Record(record.clone()));
    }
    assert_eq!(keyring.doc_spaces(doc(1)), vec![space(5), space(6)]);
    assert_eq!(keyring.doc_spaces(doc(2)), vec![space(5)]);
    assert_eq!(
        keyring.doc_records(doc(1)),
        vec![&d5, &d5_next, &d6],
        "by space, then epoch"
    );
    assert_eq!(
        keyring.attachment_spaces(attachment(1)),
        vec![space(5), space(6)]
    );
    assert!(keyring.attachment_spaces(attachment(2)).is_empty());

    // A tombstone in one space leaves the other holders; the last epoch of a space going
    // takes the space off the list.
    let purge = |space_id: SpaceId, epoch: u32| {
        KeyringItem::Tombstone(tombstone(
            space_id,
            KeySubject::Doc {
                doc_id: doc(1),
                epoch,
            },
            &a.1,
            a.0,
            T0 + 9,
        ))
    };
    keyring.merge(purge(space(5), 0));
    assert_eq!(
        keyring.doc_spaces(doc(1)),
        vec![space(5), space(6)],
        "epoch 1 of space 5 is still there"
    );
    keyring.merge(purge(space(5), 1));
    assert_eq!(keyring.doc_spaces(doc(1)), vec![space(6)]);
    assert_eq!(keyring.doc_records(doc(1)), vec![&d6]);
    keyring.merge(KeyringItem::Tombstone(tombstone(
        space(6),
        att6.subject,
        &a.1,
        a.0,
        T0 + 9,
    )));
    assert_eq!(keyring.attachment_spaces(attachment(1)), vec![space(5)]);

    keyring.remove_space(space(6));
    assert!(keyring.doc_spaces(doc(1)).is_empty());
    keyring.remove_space(space(5));
    assert!(keyring.doc_spaces(doc(2)).is_empty());
    assert!(keyring.attachment_spaces(attachment(1)).is_empty());
    // The lookups are a function of the records: an emptied keyring equals a new one.
    assert_eq!(
        keyring.items().len(),
        0,
        "only tombstones of removed spaces would be left"
    );
}

#[test]
fn rewrap_keeps_the_space_key_and_bumps_the_wrap_epoch() {
    let (a, b) = (device(1), device(2));
    let (old_ark, new_ark) = (ark(1), ark(2));
    let (space_key, record) =
        new_space_key(&old_ark, space(5), 4, &a.1, a.0, T0, &SeededRng::new(1));
    let (doc_key, doc_record) = new_doc_key(
        &space_key,
        space(5),
        doc(1),
        4,
        &a.1,
        a.0,
        T0,
        &SeededRng::new(2),
    );

    let rewrapped = rewrap_space_record(
        &record,
        &old_ark,
        &new_ark,
        &b.1,
        b.0,
        T0 + 50,
        &SeededRng::new(3),
    )
    .unwrap();
    assert_eq!(unwrap_space_key(&new_ark, &rewrapped).unwrap(), space_key);
    assert!(auth_error(unwrap_space_key(&old_ark, &rewrapped)));
    assert_eq!(rewrapped.space_id, record.space_id);
    assert_eq!(rewrapped.subject, KeySubject::Space { epoch: 4 });
    assert_eq!(rewrapped.kcv, record.kcv);
    assert_eq!(rewrapped.wrap_epoch, 1);
    assert_eq!((rewrapped.created_by, rewrapped.created_at), (b.0, T0 + 50));
    KeyringItem::Record(rewrapped.clone())
        .verify(&b.1.public().signing)
        .unwrap();
    assert!(auth_error(
        KeyringItem::Record(rewrapped.clone()).verify(&a.1.public().signing)
    ));

    // Document keys under the space key are untouched by the rotation.
    let unwrapped_space_key = unwrap_space_key(&new_ark, &rewrapped).unwrap();
    assert_eq!(
        unwrap_doc_key(&unwrapped_space_key, &doc_record).unwrap(),
        doc_key
    );

    // A second rotation counts on.
    let third = ark(3);
    let again = rewrap_space_record(
        &rewrapped,
        &new_ark,
        &third,
        &a.1,
        a.0,
        T0 + 60,
        &SeededRng::new(4),
    )
    .unwrap();
    assert_eq!(again.wrap_epoch, 2);
    assert_eq!(unwrap_space_key(&third, &again).unwrap(), space_key);
}

#[test]
fn rewrap_refuses_a_wrong_old_ark_a_wrong_kind_and_an_exhausted_epoch() {
    let a = device(1);
    let (old_ark, new_ark) = (ark(1), ark(2));
    let (space_key, record) =
        new_space_key(&old_ark, space(5), 0, &a.1, a.0, T0, &SeededRng::new(1));
    let rng = SeededRng::new(5);

    assert!(auth_error(rewrap_space_record(
        &record,
        &ark(9),
        &new_ark,
        &a.1,
        a.0,
        T0,
        &rng
    )));
    let (_, doc_record) = new_doc_key(&space_key, space(5), doc(1), 0, &a.1, a.0, T0, &rng);
    assert!(corrupted(rewrap_space_record(
        &doc_record,
        &old_ark,
        &new_ark,
        &a.1,
        a.0,
        T0,
        &rng
    )));
    let exhausted = KeyRecord {
        wrap_epoch: u32::MAX,
        ..record
    };
    assert!(corrupted(rewrap_space_record(
        &exhausted, &old_ark, &new_ark, &a.1, a.0, T0, &rng
    )));
}

// ---------------------------------------------------------------------------------
// Merge

#[test]
fn merge_is_a_union_over_spaces_and_subjects() {
    let a = device(1);
    let (sk, s5) = space_record(&a, space(5), 0, T0, 1);
    let (_, s6) = space_record(&a, space(6), 0, T0, 2);
    let (_, d1) = new_doc_key(&sk, space(5), doc(1), 0, &a.1, a.0, T0, &SeededRng::new(3));
    let (_, d2) = new_doc_key(&sk, space(5), doc(2), 0, &a.1, a.0, T0, &SeededRng::new(4));
    let (_, at) = new_attachment_key(
        &sk,
        space(5),
        attachment(1),
        0,
        &a.1,
        a.0,
        T0,
        &SeededRng::new(5),
    );
    let gone = tombstone(
        space(5),
        KeySubject::Doc {
            doc_id: doc(3),
            epoch: 0,
        },
        &a.1,
        a.0,
        T0,
    );

    let mut keyring = Keyring::new();
    for record in [&d2, &s6, &at, &s5, &d1] {
        assert!(keyring.merge(KeyringItem::Record(record.clone())));
    }
    assert!(keyring.merge(KeyringItem::Tombstone(gone.clone())));
    // Merging an item again changes nothing.
    assert!(!keyring.merge(KeyringItem::Record(d1.clone())));
    assert!(!keyring.merge(KeyringItem::Tombstone(gone.clone())));

    assert_eq!(keyring.record(space(5), &d1.subject), Some(&d1));
    assert_eq!(keyring.record(space(6), &s6.subject), Some(&s6));
    assert_eq!(keyring.record(space(6), &d1.subject), None);
    assert!(keyring.is_tombstoned(space(5), &gone.subject));
    assert!(!keyring.is_tombstoned(space(5), &d1.subject));
    assert_eq!(
        keyring.counts(),
        KeyringCounts {
            spaces: 2,
            documents: 2,
            attachments: 1,
            links: 0,
            tombstones: 1,
        }
    );

    // Records then tombstones, each in (space, subject) order.
    let items = keyring.items();
    let expected: Vec<KeyringItem> = [&s5, &d1, &d2, &at, &s6]
        .into_iter()
        .cloned()
        .map(KeyringItem::Record)
        .chain([KeyringItem::Tombstone(gone)])
        .collect();
    assert_eq!(items, expected);

    let space5: Vec<&KeyRecord> = keyring.space_records(space(5));
    assert_eq!(space5, vec![&s5, &d1, &d2, &at]);

    keyring.remove_space(space(5));
    assert_eq!(keyring.items(), vec![KeyringItem::Record(s6)]);
    assert_eq!(keyring.counts().tombstones, 0);
}

#[test]
fn a_tombstone_wins_over_every_record_in_any_order() {
    let (a, b) = (device(1), device(2));
    let (sk, _) = space_record(&a, space(5), 0, T0, 1);
    let (_, record) = new_doc_key(
        &sk,
        space(5),
        doc(1),
        0,
        &a.1,
        a.0,
        T0 + 10,
        &SeededRng::new(2),
    );
    let early = tombstone(space(5), record.subject, &b.1, b.0, T0);
    let late = tombstone(space(5), record.subject, &a.1, a.0, T0 + 20);

    let mut first = Keyring::new();
    assert!(first.merge(KeyringItem::Record(record.clone())));
    assert!(
        first.merge(KeyringItem::Tombstone(late.clone())),
        "drops the record"
    );
    assert_eq!(first.record(space(5), &record.subject), None);
    assert!(
        !first.merge(KeyringItem::Record(record.clone())),
        "ignored from now on"
    );
    assert!(
        first.merge(KeyringItem::Tombstone(early.clone())),
        "the earlier tombstone"
    );
    assert!(!first.merge(KeyringItem::Tombstone(late.clone())));

    let mut second = Keyring::new();
    assert!(second.merge(KeyringItem::Tombstone(early.clone())));
    assert!(!second.merge(KeyringItem::Tombstone(late)));
    assert!(!second.merge(KeyringItem::Record(record.clone())));
    assert_eq!(first, second);
    assert_eq!(second.items(), vec![KeyringItem::Tombstone(early)]);
    assert_eq!(second.counts().documents, 0);
}

#[test]
fn equal_wrap_epochs_keep_the_earlier_record() {
    let (a, b) = device_pair_ordered();
    let rec = |dev: &Device, at: i64, seed: u64| space_record(dev, space(5), 0, at, seed).1;

    let older = rec(&a, T0, 1);
    let newer = rec(&a, T0 + 1, 2);
    let mut keyring = Keyring::new();
    assert!(keyring.merge(KeyringItem::Record(older.clone())));
    assert!(!keyring.merge(KeyringItem::Record(newer.clone())));
    assert_eq!(keyring.record(space(5), &older.subject), Some(&older));
    let mut reversed = Keyring::new();
    assert!(reversed.merge(KeyringItem::Record(newer)));
    assert!(
        reversed.merge(KeyringItem::Record(older.clone())),
        "replaced by the earlier"
    );
    assert_eq!(reversed, keyring);

    // Same time: the smaller device id wins.
    let by_a = rec(&a, T0, 3);
    let by_b = rec(&b, T0, 4);
    for order in [[&by_a, &by_b], [&by_b, &by_a]] {
        let mut k = Keyring::new();
        for r in order {
            k.merge(KeyringItem::Record(r.clone()));
        }
        assert_eq!(k.record(space(5), &by_a.subject), Some(&by_a));
    }

    // Same time and device (two different keys): decided by the encoding, in any order.
    let twin_1 = rec(&a, T0, 5);
    let twin_2 = rec(&a, T0, 6);
    assert_ne!(twin_1, twin_2);
    let mut k1 = Keyring::new();
    k1.merge(KeyringItem::Record(twin_1.clone()));
    k1.merge(KeyringItem::Record(twin_2.clone()));
    let mut k2 = Keyring::new();
    k2.merge(KeyringItem::Record(twin_2));
    k2.merge(KeyringItem::Record(twin_1));
    assert_eq!(k1, k2);
}

#[test]
fn a_rewrapped_space_record_replaces_the_old_wrap_in_any_order() {
    let (a, b) = (device(1), device(2));
    let (old_ark, new_ark) = (ark(1), ark(2));
    let (space_key, original) =
        new_space_key(&old_ark, space(5), 0, &a.1, a.0, T0, &SeededRng::new(1));
    // Even with a clock that runs behind, the newer ARK generation wins.
    for rewrapped_at in [T0 + 100, T0 - 100] {
        let rewrapped = rewrap_space_record(
            &original,
            &old_ark,
            &new_ark,
            &b.1,
            b.0,
            rewrapped_at,
            &SeededRng::new(2),
        )
        .unwrap();
        for order in [
            [original.clone(), rewrapped.clone()],
            [rewrapped.clone(), original.clone()],
        ] {
            let mut keyring = Keyring::new();
            for record in order {
                keyring.merge(KeyringItem::Record(record));
            }
            let live = keyring.record(space(5), &original.subject).unwrap();
            assert_eq!(live, &rewrapped);
            assert_eq!(unwrap_space_key(&new_ark, live).unwrap(), space_key);
        }
        let mut keyring = Keyring::new();
        assert!(keyring.merge(KeyringItem::Record(original.clone())));
        assert!(keyring.merge(KeyringItem::Record(rewrapped.clone())));
        assert!(!keyring.merge(KeyringItem::Record(original.clone())));
    }
}

/// Two devices, the first with the smaller id.
fn device_pair_ordered() -> (Device, Device) {
    let (x, y) = (device(1), device(2));
    if x.0 < y.0 { (x, y) } else { (y, x) }
}

// ---------------------------------------------------------------------------------
// Merge: property test over random item sequences

/// A fixed pool of items with many collisions: two spaces, three subjects each; per
/// subject, records with every combination of tie-breaks (wrap epoch, time, device,
/// identical provenance) and two tombstones.
fn pool() -> &'static [KeyringItem] {
    static POOL: OnceLock<Vec<KeyringItem>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (a, b) = device_pair_ordered();
        let rng = SeededRng::new(42);
        let kek = Secret32::generate(&rng);
        let mut items = Vec::new();
        for space_id in [space(1), space(2)] {
            for subject in [
                KeySubject::Space { epoch: 0 },
                KeySubject::Doc {
                    doc_id: doc(1),
                    epoch: 0,
                },
                KeySubject::Attachment {
                    attachment_id: attachment(1),
                },
            ] {
                for (wrap_epoch, at, dev) in [
                    (0, T0 + 5, &a),
                    (0, T0 + 1, &b),
                    (0, T0 + 1, &a),
                    (0, T0 + 1, &a),
                    (1, T0 + 9, &a),
                    (1, T0 + 2, &b),
                    (2, T0 - 3, &b),
                ] {
                    let key = Secret32::generate(&rng);
                    items.push(KeyringItem::Record(
                        UnsignedRecord {
                            space_id,
                            subject,
                            wrapped_key: kb_crypto::wrap_key(
                                &key,
                                &kek,
                                &wrap_subject(space_id, &subject),
                                &rng,
                            ),
                            wrap_epoch,
                            kcv: kb_crypto::key_check_value(&key),
                            created_by: dev.0,
                            created_at: at,
                        }
                        .sign(&dev.1),
                    ));
                }
                for (at, dev) in [(T0 + 7, &a), (T0 + 3, &b)] {
                    items.push(KeyringItem::Tombstone(tombstone(
                        space_id, subject, &dev.1, dev.0, at,
                    )));
                }
            }
        }
        items
    })
}

/// The merge result computed directly from the set of items.
fn model(items: &[KeyringItem]) -> Vec<KeyringItem> {
    let mut records: BTreeMap<(SpaceId, KeySubject), KeyRecord> = BTreeMap::new();
    let mut tombstones: BTreeMap<(SpaceId, KeySubject), Tombstone> = BTreeMap::new();
    for item in items {
        let key = (item.space_id(), item.subject());
        match item {
            KeyringItem::Record(r) => {
                let best = records.entry(key).or_insert_with(|| r.clone());
                let rank = |x: &KeyRecord| {
                    (
                        core::cmp::Reverse(x.wrap_epoch),
                        x.created_at,
                        x.created_by,
                        KeyringItem::Record(x.clone()).encode(),
                    )
                };
                if rank(r) < rank(best) {
                    *best = r.clone();
                }
            }
            KeyringItem::Tombstone(t) => {
                let best = tombstones.entry(key).or_insert_with(|| t.clone());
                let rank = |x: &Tombstone| {
                    (
                        x.purged_at,
                        x.purged_by,
                        KeyringItem::Tombstone(x.clone()).encode(),
                    )
                };
                if rank(t) < rank(best) {
                    *best = t.clone();
                }
            }
        }
    }
    records.retain(|key, _| !tombstones.contains_key(key));
    records
        .into_values()
        .map(KeyringItem::Record)
        .chain(tombstones.into_values().map(KeyringItem::Tombstone))
        .collect()
}

fn merge_all<'a>(items: impl IntoIterator<Item = &'a KeyringItem>) -> Keyring {
    let mut keyring = Keyring::new();
    for item in items {
        let before = keyring.clone();
        let changed = keyring.merge(item.clone());
        assert_eq!(
            changed,
            keyring != before,
            "merge reports exactly whether it changed"
        );
    }
    keyring
}

proptest! {
    #[test]
    fn merge_is_order_independent_idempotent_and_matches_the_model(
        (order, shuffled) in proptest::collection::vec(0..pool().len(), 0..48)
            .prop_flat_map(|order| (Just(order.clone()), Just(order).prop_shuffle()))
    ) {
        let pool = pool();
        let items: Vec<&KeyringItem> = order.iter().map(|&i| &pool[i]).collect();
        let keyring = merge_all(items.iter().copied());

        let other = merge_all(shuffled.iter().map(|&i| &pool[i]));
        prop_assert_eq!(&other, &keyring);

        let owned: Vec<KeyringItem> = items.iter().map(|&i| i.clone()).collect();
        prop_assert_eq!(keyring.items(), model(&owned));

        let mut again = keyring.clone();
        for item in &owned {
            prop_assert!(!again.merge(item.clone()));
        }
        prop_assert_eq!(&again, &keyring);

        // Rebuilding from the merged items gives the same keyring (what the core does
        // when it loads the stored rows).
        let reloaded = merge_all(keyring.items().iter());
        prop_assert_eq!(&reloaded, &keyring);
    }
}

// ---------------------------------------------------------------------------------
// Transport sealing

#[test]
fn sealed_items_round_trip() {
    let a = device(1);
    let root = ark(1);
    let (sk, record) = space_record(&a, space(5), 0, T0, 1);
    let (_, doc_record) = new_doc_key(&sk, space(5), doc(2), 0, &a.1, a.0, T0, &SeededRng::new(2));
    let items = [
        KeyringItem::Record(record),
        KeyringItem::Record(doc_record),
        KeyringItem::Tombstone(tombstone(
            space(5),
            KeySubject::Space { epoch: 1 },
            &a.1,
            a.0,
            T0,
        )),
    ];
    let rng = SeededRng::new(3);
    for item in items {
        let sealed = seal_item(&item, &root, &rng);
        assert_eq!(open_item(&sealed, &root).unwrap(), item);

        let envelope = Envelope::decode(&sealed).unwrap();
        assert_eq!(envelope.kind, EnvelopeKind::Keyring);
        assert_eq!(
            envelope.header,
            wrap_subject(item.space_id(), &item.subject())
        );
        assert!(envelope.signature.is_none());
        let key = kb_crypto::derive_key(root.secret(), Label::KEYRING, b"");
        let (_, plaintext) = kb_crypto::open_envelope(&sealed, &key, None).unwrap();
        assert_eq!(plaintext.as_slice(), item.encode().as_slice());

        assert_ne!(seal_item(&item, &root, &rng), sealed, "fresh nonce");
    }
}

#[test]
fn sealed_items_need_the_ark_and_detect_tampering() {
    let a = device(1);
    let root = ark(1);
    let (_, record) = space_record(&a, space(5), 0, T0, 1);
    let sealed = seal_item(&KeyringItem::Record(record), &root, &SeededRng::new(3));

    assert!(auth_error(open_item(&sealed, &ark(2))));
    for i in 0..sealed.len() {
        let mut tampered = sealed.clone();
        tampered[i] ^= 0x04;
        assert!(open_item(&tampered, &root).is_err(), "byte {i}");
    }
    for len in 0..sealed.len() {
        assert!(open_item(&sealed[..len], &root).is_err(), "{len}");
    }
}

#[test]
fn open_item_refuses_mismatched_headers_kinds_signatures_and_garbage() {
    let a = device(1);
    let root = ark(1);
    let key = kb_crypto::derive_key(root.secret(), Label::KEYRING, b"");
    let rng = SeededRng::new(3);
    let (_, record) = space_record(&a, space(5), 0, T0, 1);
    let item = KeyringItem::Record(record);
    let header = wrap_subject(space(5), &item.subject());

    let other_header = wrap_subject(space(6), &item.subject());
    let mismatched = kb_crypto::seal_envelope(
        EnvelopeKind::Keyring,
        &other_header,
        &key,
        &item.encode(),
        None,
        &rng,
    );
    assert!(corrupted(open_item(&mismatched, &root)));

    let other_kind = kb_crypto::seal_envelope(
        EnvelopeKind::Entry,
        &header,
        &key,
        &item.encode(),
        None,
        &rng,
    );
    assert!(corrupted(open_item(&other_kind, &root)));

    let signed = kb_crypto::seal_envelope(
        EnvelopeKind::Keyring,
        &header,
        &key,
        &item.encode(),
        Some(a.1.signing()),
        &rng,
    );
    assert!(corrupted(open_item(&signed, &root)));

    let garbage = kb_crypto::seal_envelope(
        EnvelopeKind::Keyring,
        &header,
        &key,
        b"not an item",
        None,
        &rng,
    );
    assert!(corrupted(open_item(&garbage, &root)));
}

// ---------------------------------------------------------------------------------
// merge_ordered: the keyring of a shared feed (M13, ADR-0044)

fn doc_record(dev: &Device, doc_id: DocId, epoch: u32, at: i64, seed: u64) -> KeyRecord {
    new_doc_key(
        &Secret32::from_bytes([7; 32]),
        space(5),
        doc_id,
        epoch,
        &dev.1,
        dev.0,
        at,
        &SeededRng::new(seed),
    )
    .1
}

fn doc_tombstone(dev: &Device, doc_id: DocId, epoch: u32, at: i64) -> Tombstone {
    tombstone(
        space(5),
        KeySubject::Doc { doc_id, epoch },
        &dev.1,
        dev.0,
        at,
    )
}

/// Every device may sign tombstones: nobody is an Owner.
fn nobody(_: &Tombstone) -> bool {
    false
}

#[test]
fn the_first_record_in_feed_order_wins_whatever_its_epoch_or_time() {
    let (a, b) = (device(1), device(2));
    let subject = KeySubject::Doc {
        doc_id: doc(1),
        epoch: 2,
    };
    // `first` is later and has the lower wrap epoch (in `record_wins` terms it loses to `second`).
    let first = doc_record(&a, doc(1), 2, T0 + 100, 1);
    let mut second = doc_record(&b, doc(1), 2, T0, 2);
    second.wrap_epoch = 9;
    let mut keyring = Keyring::new();
    assert!(keyring.merge_ordered(KeyringItem::Record(first.clone()), &nobody));
    assert!(!keyring.merge_ordered(KeyringItem::Record(second.clone()), &nobody));
    assert_eq!(keyring.record(space(5), &subject), Some(&first));

    // The plain merge would have preferred the other one: the rule is not the personal one.
    let mut personal = Keyring::new();
    personal.merge(KeyringItem::Record(first.clone()));
    personal.merge(KeyringItem::Record(second.clone()));
    assert_eq!(personal.record(space(5), &subject), Some(&second));

    // The feed order decides.
    let mut reversed = Keyring::new();
    assert!(reversed.merge_ordered(KeyringItem::Record(second.clone()), &nobody));
    assert!(!reversed.merge_ordered(KeyringItem::Record(first), &nobody));
    assert_eq!(reversed.record(space(5), &subject), Some(&second));
    assert_ne!(keyring, reversed);
}

#[test]
fn a_backdated_record_does_not_displace_the_first_one() {
    let (honest, hostile) = (device(1), device(2));
    let honest_record = doc_record(&honest, doc(1), 1, T0, 1);
    let backdated = doc_record(&hostile, doc(1), 1, T0 - 86_400_000, 2);
    let mut keyring = Keyring::new();
    keyring.merge_ordered(KeyringItem::Record(honest_record.clone()), &nobody);
    assert!(!keyring.merge_ordered(KeyringItem::Record(backdated), &nobody));
    assert_eq!(
        keyring.record(space(5), &honest_record.subject),
        Some(&honest_record)
    );
    // The same item again changes nothing.
    assert!(!keyring.merge_ordered(KeyringItem::Record(honest_record), &nobody));
}

#[test]
fn only_a_tombstone_of_an_owner_counts_and_the_first_one_stays() {
    let (owner, member, other_owner) = (device(1), device(2), device(3));
    let is_owner = |t: &Tombstone| t.purged_by == owner.0 || t.purged_by == other_owner.0;
    let subject = KeySubject::Doc {
        doc_id: doc(1),
        epoch: 1,
    };
    let record = doc_record(&member, doc(1), 1, T0, 1);

    let mut keyring = Keyring::new();
    keyring.merge_ordered(KeyringItem::Record(record.clone()), &is_owner);
    // A member's tombstone changes nothing.
    let by_member = doc_tombstone(&member, doc(1), 1, T0 + 1);
    assert!(!keyring.merge_ordered(KeyringItem::Tombstone(by_member), &is_owner));
    assert_eq!(keyring.record(space(5), &subject), Some(&record));
    assert!(!keyring.is_tombstoned(space(5), &subject));

    // An Owner's does: the record goes, and the subject stays purged.
    let by_owner = doc_tombstone(&owner, doc(1), 1, T0 + 2);
    assert!(keyring.merge_ordered(KeyringItem::Tombstone(by_owner.clone()), &is_owner));
    assert_eq!(keyring.record(space(5), &subject), None);
    assert!(keyring.is_tombstoned(space(5), &subject));
    assert_eq!(keyring.counts().documents, 0);
    assert!(keyring.doc_spaces(doc(1)).is_empty(), "the index follows");

    // Later records and later tombstones change nothing; the first tombstone stays.
    assert!(!keyring.merge_ordered(KeyringItem::Record(record), &is_owner));
    let later = doc_tombstone(&other_owner, doc(1), 1, T0 - 5);
    assert!(!keyring.merge_ordered(KeyringItem::Tombstone(later), &is_owner));
    assert_eq!(keyring.items(), vec![KeyringItem::Tombstone(by_owner)]);
}

#[test]
fn a_tombstone_that_comes_first_blocks_every_later_record() {
    let (owner, member) = (device(1), device(2));
    let is_owner = |t: &Tombstone| t.purged_by == owner.0;
    let mut keyring = Keyring::new();
    assert!(keyring.merge_ordered(
        KeyringItem::Tombstone(doc_tombstone(&owner, doc(1), 1, T0)),
        &is_owner
    ));
    assert!(!keyring.merge_ordered(
        KeyringItem::Record(doc_record(&member, doc(1), 1, T0 + 1, 1)),
        &is_owner
    ));
    assert_eq!(keyring.counts().documents, 0);
    assert_eq!(keyring.counts().tombstones, 1);
    // Another subject of the same document is unaffected.
    assert!(keyring.merge_ordered(
        KeyringItem::Record(doc_record(&member, doc(1), 2, T0 + 1, 2)),
        &is_owner
    ));
}

#[test]
fn attachments_follow_the_same_rules_and_the_holders_index_stays_right() {
    let (owner, member) = (device(1), device(2));
    let is_owner = |t: &Tombstone| t.purged_by == owner.0;
    let (_, record) = new_attachment_key(
        &Secret32::from_bytes([7; 32]),
        space(5),
        attachment(3),
        0,
        &member.1,
        member.0,
        T0,
        &SeededRng::new(1),
    );
    let mut keyring = Keyring::new();
    assert!(keyring.merge_ordered(KeyringItem::Record(record.clone()), &is_owner));
    assert_eq!(keyring.attachment_spaces(attachment(3)), vec![space(5)]);
    assert!(keyring.merge_ordered(
        KeyringItem::Tombstone(tombstone(
            space(5),
            record.subject,
            &owner.1,
            owner.0,
            T0 + 1
        )),
        &is_owner
    ));
    assert!(keyring.attachment_spaces(attachment(3)).is_empty());
}

#[test]
fn space_records_never_enter_through_a_shared_feed() {
    let member = device(2);
    let (_, record) = space_record(&member, space(5), 3, T0, 1);
    let everyone = |_: &Tombstone| true;
    let mut keyring = Keyring::new();
    assert!(!keyring.merge_ordered(KeyringItem::Record(record.clone()), &everyone));
    assert!(!keyring.merge_ordered(
        KeyringItem::Tombstone(tombstone(space(5), record.subject, &member.1, member.0, T0)),
        &everyone
    ));
    assert_eq!(keyring, Keyring::new());
    // The personal keyring still takes them.
    assert!(keyring.merge(KeyringItem::Record(record)));
}

/// A pool of items for one shared space: two documents at two epochs, written by two members and
/// an Owner, with times that disagree with any feed order, and tombstones by the Owner and by a
/// member.
fn shared_pool() -> &'static [KeyringItem] {
    static POOL: OnceLock<Vec<KeyringItem>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (owner, m1, m2) = (device(1), device(2), device(3));
        let mut items = Vec::new();
        let mut seed = 0u64;
        for d in 1u8..=2 {
            for epoch in 1u32..=2 {
                for (writer, at) in [(&m1, T0 + 50), (&m2, T0), (&owner, T0 + 25)] {
                    seed += 1;
                    items.push(KeyringItem::Record(doc_record(
                        writer,
                        doc(d),
                        epoch,
                        at,
                        seed,
                    )));
                }
                items.push(KeyringItem::Tombstone(doc_tombstone(
                    &owner,
                    doc(d),
                    epoch,
                    T0 + 10,
                )));
                items.push(KeyringItem::Tombstone(doc_tombstone(
                    &m1,
                    doc(d),
                    epoch,
                    T0 + 5,
                )));
            }
        }
        items
    })
}

/// The rule written out naively over an ordered sequence.
fn ordered_model(
    sequence: &[&KeyringItem],
    is_owner: &dyn Fn(&Tombstone) -> bool,
) -> Vec<KeyringItem> {
    let mut records: BTreeMap<(SpaceId, KeySubject), KeyRecord> = BTreeMap::new();
    let mut tombstones: BTreeMap<(SpaceId, KeySubject), Tombstone> = BTreeMap::new();
    for item in sequence {
        let key = (item.space_id(), item.subject());
        match item {
            KeyringItem::Record(record) => {
                if !tombstones.contains_key(&key) {
                    records.entry(key).or_insert_with(|| record.clone());
                }
            }
            KeyringItem::Tombstone(tombstone) => {
                if is_owner(tombstone) && !tombstones.contains_key(&key) {
                    records.remove(&key);
                    tombstones.insert(key, tombstone.clone());
                }
            }
        }
    }
    records
        .into_values()
        .map(KeyringItem::Record)
        .chain(tombstones.into_values().map(KeyringItem::Tombstone))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn ordered_merging_follows_the_first_in_feed_order_model(
        picks in proptest::collection::vec(0usize..24, 0..40),
    ) {
        let pool = shared_pool();
        let owner = device(1);
        let is_owner = |t: &Tombstone| t.purged_by == owner.0;
        let sequence: Vec<&KeyringItem> = picks.iter().map(|i| &pool[i % pool.len()]).collect();

        let mut keyring = Keyring::new();
        for item in &sequence {
            keyring.merge_ordered((*item).clone(), &is_owner);
        }
        prop_assert_eq!(keyring.items(), ordered_model(&sequence, &is_owner));

        // The holders index agrees with the records.
        for d in 1u8..=2 {
            let has_record = keyring
                .items()
                .iter()
                .any(|i| matches!(i, KeyringItem::Record(r) if r.subject.encode()[1..17] == [d; 16]));
            prop_assert_eq!(!keyring.doc_spaces(doc(d)).is_empty(), has_record);
        }

        // Offering the same sequence again changes nothing (idempotent per item).
        let before = keyring.clone();
        for item in &sequence {
            prop_assert!(!keyring.merge_ordered((*item).clone(), &is_owner));
        }
        prop_assert_eq!(keyring, before);
    }
}
