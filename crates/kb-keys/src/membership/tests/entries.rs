//! One entry: build, encode, decode, hash, signature, private part (real cryptography).

use super::*;
use kb_crypto::{CryptoError, SIGNATURE_LEN};

/// A real two-person space with the `Create` built (not yet folded).
fn built() -> (Space, MembershipEntry) {
    let mut space = Space::new(2, false, 7);
    let create = space.create(0);
    (space, create)
}

fn signing_key(space: &Space, person: usize) -> VerifyingKey {
    space.world.people[person].devices[0].keys.public().signing
}

fn meta(space: &Space, epoch: u32) -> Secret32 {
    membership_meta_key(&space.keys[&epoch], space.feed)
}

use crate::space_keys::membership_meta_key;

#[test]
fn an_entry_is_one_signed_membership_envelope() {
    let (space, entry) = built();
    let bytes = entry.encode();
    // The envelope is of kind Membership (9), its header is the clear header, it is signed.
    assert_eq!(entry.envelope.kind, EnvelopeKind::Membership);
    assert_eq!(bytes[..3], [1, 1, 9]);
    assert_eq!(entry.envelope.header, entry.header.encode());
    assert!(entry.envelope.signature.is_some());
    assert_eq!(entry.kind(), EntryKind::Create);

    // decode(encode) is the identity and the encoding is canonical.
    let decoded = MembershipEntry::decode(&bytes).unwrap();
    assert_eq!(decoded, entry);
    assert_eq!(decoded.encode(), bytes);
    // peek reads the header without anything else.
    assert_eq!(MembershipEntry::peek(&bytes).unwrap(), entry.header);

    // The hash is BLAKE3 of the encoding and names the entry's place.
    assert_eq!(entry.hash(), kb_crypto::hash(&bytes));
    assert_eq!(
        entry.head(),
        ChainHead {
            seq: 0,
            hash: kb_crypto::hash(&bytes)
        }
    );
    assert_eq!(entry.header.feed, space.feed);
}

#[test]
fn the_signature_is_the_device_over_the_envelope_with_the_membership_context() {
    let (space, entry) = built();
    let key = signing_key(&space, 0);
    entry.verify_signature(&key).unwrap();

    // By hand: every byte of the encoding before the signature, context `envelope/membership`.
    let bytes = entry.encode();
    let (signed, signature) = bytes.split_at(bytes.len() - SIGNATURE_LEN);
    let signature = Signature::from_bytes(signature).unwrap();
    kb_crypto::verify(&key, "envelope/membership", signed, &signature).unwrap();
    assert_eq!(EnvelopeKind::Membership.context(), "envelope/membership");

    // Another device, another context and a missing signature fail.
    let stranger = signing_key(&space, 1);
    assert!(matches!(
        entry.verify_signature(&stranger),
        Err(KeysError::Crypto(CryptoError::Authentication))
    ));
    assert!(kb_crypto::verify(&key, "envelope/entry", signed, &signature).is_err());
    let mut unsigned = entry.clone();
    unsigned.envelope.signature = None;
    assert!(matches!(
        unsigned.verify_signature(&key),
        Err(KeysError::Crypto(CryptoError::Authentication))
    ));
    let mut wrong_kind = entry;
    wrong_kind.envelope.kind = EnvelopeKind::Entry;
    assert!(wrong_kind.verify_signature(&key).is_err());
}

#[test]
fn a_changed_byte_is_refused_by_decode_or_by_the_signature() {
    let (space, entry) = built();
    let key = signing_key(&space, 0);
    let bytes = entry.encode();
    for i in (0..bytes.len()).step_by(5).chain([bytes.len() - 1]) {
        let mut other = bytes.clone();
        other[i] ^= 0x01;
        match MembershipEntry::decode(&other) {
            Err(_) => {}
            Ok(decoded) => assert!(decoded.verify_signature(&key).is_err(), "byte {i}"),
        }
    }
}

#[test]
fn decoding_an_entry_is_strict() {
    let (_, entry) = built();
    let bytes = entry.encode();
    for len in (0..bytes.len()).step_by(3) {
        assert!(
            MembershipEntry::decode(&bytes[..len]).is_err(),
            "cut at {len}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(MembershipEntry::decode(&longer).is_err());

    // An envelope of another kind, an unsigned one and one whose header is not canonical.
    let mut other_kind = entry.envelope.clone();
    other_kind.kind = EnvelopeKind::Entry;
    assert!(MembershipEntry::decode(&other_kind.encode()).is_err());
    let mut unsigned = entry.envelope.clone();
    unsigned.signature = None;
    assert!(MembershipEntry::decode(&unsigned.encode()).is_err());
    let mut padded = entry.envelope.clone();
    padded.header.push(0);
    assert!(MembershipEntry::decode(&padded.encode()).is_err());
    let mut garbled = entry.envelope;
    garbled.header[0] = 9;
    assert!(MembershipEntry::decode(&garbled.encode()).is_err());
    assert!(MembershipEntry::peek(b"not an entry").is_err());
}

#[test]
fn the_private_part_opens_under_the_membership_meta_key_of_its_epoch() {
    let (space, entry) = built();
    let key = meta(&space, 1);
    assert_eq!(
        entry.open_private(&key).unwrap(),
        Private::Create {
            space_name: "Space".to_owned(),
            settings: SpaceSettings::default(),
            profile_name: "Person 0".to_owned(),
        }
    );
    // A wrong key, tampering with the ciphertext, the nonce or the header (the AAD).
    let wrong = Secret32::from_bytes([1; 32]);
    assert!(matches!(
        entry.open_private(&wrong),
        Err(KeysError::Crypto(CryptoError::Authentication))
    ));
    let mut tampered = entry.clone();
    tampered.envelope.ciphertext[0] ^= 1;
    assert!(tampered.open_private(&key).is_err());
    let mut tampered = entry.clone();
    tampered.envelope.header[40] ^= 1;
    assert!(tampered.open_private(&key).is_err());
    let mut tampered = entry;
    tampered.envelope.nonce = kb_crypto::Nonce24::from_bytes([3; 24]);
    assert!(tampered.open_private(&key).is_err());
}

#[test]
fn the_private_part_must_match_the_kind() {
    let mut space = Space::new(2, false, 8);
    space.start(0, &[(1, MemberRole::Editor)]);
    let key = meta(&space, 1);
    let who = Space::who(0);
    // A Profile entry that seals the private part of a Join.
    let entry = space.entry(
        who,
        1,
        EntryBody::Profile,
        &Private::Join {
            profile_name: "x".to_owned(),
        },
    );
    assert!(matches!(
        entry.open_private(&key),
        Err(KeysError::Corrupted(_))
    ));
    // A Leave that seals something.
    let entry = space.entry(
        who,
        1,
        EntryBody::Leave,
        &Private::Profile {
            display_name: "x".to_owned(),
        },
    );
    assert!(matches!(
        entry.open_private(&key),
        Err(KeysError::Corrupted(_))
    ));
    // The kinds without private fields seal an empty plaintext: the sealed box is the tag only.
    let entry = space.entry(who, 1, EntryBody::Close, &Private::None);
    assert_eq!(entry.envelope.ciphertext.len(), kb_crypto::TAG_LEN);
    assert_eq!(entry.open_private(&key).unwrap(), Private::None);
}

#[test]
fn every_private_variant_round_trips_through_an_entry() {
    let mut space = Space::new(2, false, 9);
    space.start(0, &[(1, MemberRole::Editor)]);
    let key = meta(&space, 1);
    let cases = [
        (
            EntryBody::Profile,
            Private::Profile {
                display_name: "Ann \u{1F4F1}".to_owned(),
            },
        ),
        (
            EntryBody::Settings,
            Private::Settings {
                settings: SpaceSettings {
                    trash_retention_days: 0,
                    editors_may_link: true,
                },
            },
        ),
        (EntryBody::Leave, Private::None),
    ];
    for (body, private) in cases {
        let entry = space.entry(Space::who(0), 1, body, &private);
        let decoded = MembershipEntry::decode(&entry.encode()).unwrap();
        assert_eq!(decoded.open_private(&key).unwrap(), private);
    }
}

#[test]
fn the_server_reads_a_header_without_any_key() {
    // What a server routes on is in clear: kind, signer, position, boxes, invite fields.
    let mut space = Space::new(2, false, 10);
    space.start(0, &[]);
    let (invite, _) = space.invite(0, MemberRole::Viewer, None, DAY);
    let peeked = MembershipEntry::peek(&invite.encode()).unwrap();
    assert_eq!(peeked.kind(), EntryKind::Invite);
    assert_eq!(peeked.seq, 1);
    assert_eq!(peeked.account, space.account(0));
    assert!(matches!(
        peeked.body,
        EntryBody::Invite {
            role: MemberRole::Viewer,
            ..
        }
    ));
    // The names and the settings are not in the clear part.
    let bytes = invite.encode();
    assert!(!bytes.windows(6).any(|w| w == b"Person"));
}
