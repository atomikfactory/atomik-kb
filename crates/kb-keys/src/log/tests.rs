//! Device log: encoding, builders, verification rules, account id, device names.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::{ark, device};
use kb_crypto::{Label, Secret32};
use kb_types::testing::SeededRng;

const T0: i64 = 1_760_000_000_000;

type Device = (DeviceId, DeviceKeys);

/// A recovery signing key from a fixed seed.
fn recovery(seed: u8) -> SigningKey {
    SigningKey::from_seed(&Secret32::from_bytes([seed; 32]))
}

/// Stand-in for an encrypted name (the log treats it as opaque bytes).
fn name(tag: u8) -> Vec<u8> {
    vec![tag; 12]
}

/// Genesis by `first`, recovery key `kit`.
fn genesis(first: &Device, kit: &SigningKey) -> DeviceLog {
    DeviceLog::genesis(first.0, &first.1, kit.verifying_key(), name(1), T0).unwrap()
}

fn add_by(log: &mut DeviceLog, new: &Device, by: &Device, at: i64) -> LogEntry {
    log.add_device(
        new.0,
        new.1.public(),
        name(2),
        LogSigner::Device(&by.1, by.0),
        at,
    )
    .unwrap()
    .clone()
}

/// A correctly chained next entry for `log`, signed by `key`, with no rule checked.
fn next_entry(log: &DeviceLog, body: LogEntryBody, key: &SigningKey) -> LogEntry {
    let seq = u64::try_from(log.entries().len()).unwrap();
    LogEntry::signed(seq, log.head_hash().unwrap_or([0; 32]), body, key)
}

fn added_body(new: &Device, added_by: Signer, via_kit: bool) -> LogEntryBody {
    LogEntryBody::DeviceAdded {
        device_id: new.0,
        keys: new.1.public(),
        encrypted_name: name(3),
        added_by,
        via_kit,
        at: T0 + 50,
    }
}

/// `entry` after `log` fails with `expected` through `append` (log unchanged), through
/// `from_entries`, and through the stored (encoded) form.
fn assert_rejected(log: &DeviceLog, entry: &LogEntry, expected: &LogError) {
    let mut copy = log.clone();
    assert_eq!(&copy.append(entry.clone()).unwrap_err(), expected);
    assert_eq!(&copy, log, "a rejected entry leaves the log unchanged");

    let mut entries = log.entries().to_vec();
    entries.push(entry.clone());
    assert_eq!(
        &DeviceLog::from_entries(entries.clone()).unwrap_err(),
        expected
    );
    if entries.iter().all(|e| e.version == LOG_ENTRY_VERSION) {
        let stored: Vec<LogEntry> = entries
            .iter()
            .map(|e| LogEntry::decode(&e.encode()).unwrap())
            .collect();
        assert_eq!(&DeviceLog::from_entries(stored).unwrap_err(), expected);
    }
}

/// Every entry kind and both signer kinds: Genesis(A); A adds B; the kit adds C;
/// C revokes B; A rotates the kit to `kit2`; `kit2` adds D.
struct Story {
    a: Device,
    b: Device,
    c: Device,
    d: Device,
    kit: SigningKey,
    kit2: SigningKey,
    log: DeviceLog,
}

fn story() -> Story {
    let (a, b, c, d) = (device(1), device(2), device(3), device(4));
    let (kit, kit2) = (recovery(7), recovery(8));
    let mut log = genesis(&a, &kit);
    add_by(&mut log, &b, &a, T0 + 1);
    log.add_device(
        c.0,
        c.1.public(),
        name(3),
        LogSigner::Recovery(&kit),
        T0 + 2,
    )
    .unwrap();
    log.revoke_device(b.0, (&c.1, c.0), T0 + 3).unwrap();
    log.rotate_recovery(kit2.verifying_key(), (&a.1, a.0), T0 + 4)
        .unwrap();
    log.add_device(
        d.0,
        d.1.public(),
        name(4),
        LogSigner::Recovery(&kit2),
        T0 + 5,
    )
    .unwrap();
    Story {
        a,
        b,
        c,
        d,
        kit,
        kit2,
        log,
    }
}

#[test]
fn genesis_verifies_and_describes_the_first_device() {
    let a = device(1);
    let kit = recovery(7);
    let log = genesis(&a, &kit);

    assert_eq!(log.entries().len(), 1);
    let entry = &log.entries()[0];
    assert_eq!(entry.version, LOG_ENTRY_VERSION);
    assert_eq!(entry.seq, 0);
    assert_eq!(entry.prev_hash, [0; 32]);
    assert_eq!(log.head_hash(), Some(entry.hash()));

    let state = log.verify().unwrap();
    assert_eq!(state.account_id, account_id_of(entry));
    assert_eq!(state.recovery_key, kit.verifying_key());
    assert_eq!(
        state.devices,
        vec![DeviceRecord {
            device_id: a.0,
            keys: a.1.public(),
            encrypted_name: name(1),
            added_at: T0,
            added_via_kit: false,
            added_by: Signer::Device(a.0),
            revoked_by: None,
            revoked_at: None,
        }]
    );
    assert_eq!(state.active(a.0), state.devices.first());
}

#[test]
fn builders_produce_a_chained_log_that_verifies() {
    let s = story();
    let entries = s.log.entries();
    assert_eq!(entries.len(), 6);
    let mut prev = [0u8; 32];
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry.seq, u64::try_from(i).unwrap());
        assert_eq!(entry.prev_hash, prev);
        prev = entry.hash();
    }
    assert_eq!(s.log.head_hash(), Some(prev));

    assert!(matches!(
        entries[1].body,
        LogEntryBody::DeviceAdded {
            added_by: Signer::Device(by),
            via_kit: false,
            ..
        } if by == s.a.0
    ));
    assert!(matches!(
        entries[2].body,
        LogEntryBody::DeviceAdded {
            added_by: Signer::Recovery,
            via_kit: true,
            ..
        }
    ));

    let state = s.log.verify().unwrap();
    assert_eq!(state.account_id, account_id_of(&entries[0]));
    assert_eq!(state.recovery_key, s.kit2.verifying_key());
    assert_ne!(state.recovery_key, s.kit.verifying_key());
    let ids: Vec<DeviceId> = state.devices.iter().map(|d| d.device_id).collect();
    assert_eq!(ids, [s.a.0, s.b.0, s.c.0, s.d.0], "log order");
    assert_eq!(state.devices[1].revoked_at, Some(T0 + 3));
    assert!(state.active(s.b.0).is_none());
    assert!(state.active(s.a.0).is_some());
    assert_eq!(
        state
            .devices
            .iter()
            .map(|d| d.added_via_kit)
            .collect::<Vec<_>>(),
        [false, false, true, true]
    );
    assert_eq!(state.devices[3].added_at, T0 + 5);
    assert_eq!(state.devices[3].keys, s.d.1.public());
    assert_eq!(state.devices[3].encrypted_name, name(4));

    // Stored entries (encoded, decoded) rebuild the same log.
    let stored: Vec<LogEntry> = entries
        .iter()
        .map(|e| LogEntry::decode(&e.encode()).unwrap())
        .collect();
    assert_eq!(DeviceLog::from_entries(stored).unwrap(), s.log);
}

#[test]
fn encoding_round_trips_every_variant_and_signer() {
    let s = story();
    for entry in s.log.entries() {
        let bytes = entry.encode();
        assert_eq!(LogEntry::decode(&bytes).unwrap(), *entry);
        let signed = entry.signed_bytes();
        assert_eq!(signed.len() + SIGNATURE_LEN, bytes.len());
        assert_eq!(&bytes[..signed.len()], signed.as_slice());
        assert_eq!(
            &bytes[signed.len()..],
            entry.signature.to_bytes().as_slice()
        );
        assert_eq!(entry.hash(), kb_crypto::hash(&bytes));
    }

    // The Genesis is signed by its device over the signed bytes, in the log context only.
    let first = &s.log.entries()[0];
    let key = s.a.1.public().signing;
    let signed = first.signed_bytes();
    assert!(kb_crypto::verify(&key, LOG_CONTEXT, &signed, &first.signature).is_ok());
    assert!(kb_crypto::verify(&key, "keyring", &signed, &first.signature).is_err());
    assert!(kb_crypto::verify(&key, LOG_CONTEXT, &first.encode(), &first.signature).is_err());
}

#[test]
fn encoding_layout_is_the_documented_one() {
    let a = device(1);
    let b = device(2);
    let kit = recovery(7);
    let keys = a.1.public();
    let prev = [0x5A; 32];

    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&3u64.to_le_bytes());
    header.extend_from_slice(&prev);

    let check = |body: LogEntryBody, expected_body: Vec<u8>| {
        let entry = LogEntry::signed(3, prev, body, b.1.signing());
        let mut expected = header.clone();
        expected.extend_from_slice(&expected_body);
        assert_eq!(entry.signed_bytes(), expected);
        let mut encoded = expected;
        encoded.extend_from_slice(&entry.signature.to_bytes());
        assert_eq!(entry.encode(), encoded);
    };

    let mut genesis = vec![1u8];
    genesis.extend_from_slice(a.0.as_bytes());
    genesis.extend_from_slice(&keys.signing.to_bytes());
    genesis.extend_from_slice(&keys.agreement.to_bytes());
    genesis.extend_from_slice(&kit.verifying_key().to_bytes());
    genesis.extend_from_slice(&[3, 0, 0, 0, 0xE1, 0xE2, 0xE3]);
    genesis.extend_from_slice(&(-5i64).to_le_bytes());
    check(
        LogEntryBody::Genesis {
            device_id: a.0,
            keys,
            recovery_key: kit.verifying_key(),
            encrypted_name: vec![0xE1, 0xE2, 0xE3],
            at: -5,
        },
        genesis,
    );

    for (added_by, signer_bytes, via_kit) in [
        (
            Signer::Device(b.0),
            [&[1u8][..], b.0.as_bytes()].concat(),
            0u8,
        ),
        (Signer::Recovery, vec![2u8], 1u8),
    ] {
        let mut added = vec![2u8];
        added.extend_from_slice(a.0.as_bytes());
        added.extend_from_slice(&keys.signing.to_bytes());
        added.extend_from_slice(&keys.agreement.to_bytes());
        added.extend_from_slice(&[0, 0, 0, 0]);
        added.extend_from_slice(&signer_bytes);
        added.push(via_kit);
        added.extend_from_slice(&0x0102_0304_0506_0708i64.to_le_bytes());
        check(
            LogEntryBody::DeviceAdded {
                device_id: a.0,
                keys,
                encrypted_name: Vec::new(),
                added_by,
                via_kit: via_kit == 1,
                at: 0x0102_0304_0506_0708,
            },
            added,
        );
    }

    let mut revoked = vec![3u8];
    revoked.extend_from_slice(a.0.as_bytes());
    revoked.extend_from_slice(b.0.as_bytes());
    revoked.extend_from_slice(&7i64.to_le_bytes());
    check(
        LogEntryBody::DeviceRevoked {
            device_id: a.0,
            revoked_by: b.0,
            at: 7,
        },
        revoked,
    );

    let mut rotated = vec![4u8];
    rotated.extend_from_slice(&kit.verifying_key().to_bytes());
    rotated.extend_from_slice(b.0.as_bytes());
    rotated.extend_from_slice(&8i64.to_le_bytes());
    check(
        LogEntryBody::RecoveryRotated {
            recovery_key: kit.verifying_key(),
            by: b.0,
            at: 8,
        },
        rotated,
    );

    for boxes in [Vec::new(), vec![(a.0, vec![0xB1; 3]), (b.0, Vec::new())]] {
        let mut root = vec![5u8];
        root.extend_from_slice(&0x0A0B_0C0Du32.to_le_bytes());
        root.extend_from_slice(&u32::try_from(boxes.len()).unwrap().to_le_bytes());
        for (device_id, sealed) in &boxes {
            root.extend_from_slice(device_id.as_bytes());
            root.extend_from_slice(&u32::try_from(sealed.len()).unwrap().to_le_bytes());
            root.extend_from_slice(sealed);
        }
        root.extend_from_slice(a.0.as_bytes());
        root.extend_from_slice(&9i64.to_le_bytes());
        check(
            LogEntryBody::RootRotated {
                generation: 0x0A0B_0C0D,
                boxes,
                by: a.0,
                at: 9,
            },
            root,
        );
    }
}

#[test]
fn decode_rejects_malformed_bytes() {
    let s = story();
    let is_encoding = |r: Result<LogEntry, LogError>| matches!(r, Err(LogError::Encoding(_)));

    for entry in s.log.entries() {
        let bytes = entry.encode();
        for len in 0..bytes.len() {
            assert!(
                is_encoding(LogEntry::decode(&bytes[..len])),
                "truncated to {len}"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(is_encoding(LogEntry::decode(&longer)), "trailing byte");
    }

    let bytes = s.log.entries()[1].encode();
    // Unknown versions.
    for version in [0u16, 2, 0x0100, u16::MAX] {
        let mut other = bytes.clone();
        other[..2].copy_from_slice(&version.to_le_bytes());
        let err = LogEntry::decode(&other).unwrap_err();
        assert_eq!(
            err,
            LogError::Encoding(format!("unsupported entry version {version}"))
        );
    }
    // Unknown body tags (offset 2 + 8 + 32).
    for tag in [0u8, 6, 0xFF] {
        let mut other = bytes.clone();
        other[42] = tag;
        assert!(is_encoding(LogEntry::decode(&other)), "body tag {tag}");
    }
    // DeviceAdded: signer tag after tag, id, two keys and the 12-byte name.
    let signer_at = 43 + 16 + 32 + 32 + 4 + 12;
    assert_eq!(bytes[signer_at], 1, "entry 1 is added by a device");
    for tag in [0u8, 3, 0xFF] {
        let mut other = bytes.clone();
        other[signer_at] = tag;
        assert!(is_encoding(LogEntry::decode(&other)), "signer tag {tag}");
    }
    // via_kit must be 0 or 1.
    let via_kit_at = signer_at + 1 + 16;
    assert_eq!(bytes[via_kit_at], 0);
    for value in [2u8, 0xFF] {
        let mut other = bytes.clone();
        other[via_kit_at] = value;
        assert!(is_encoding(LogEntry::decode(&other)), "via_kit {value}");
    }
    // A name length pointing past the end.
    let mut other = bytes.clone();
    other[signer_at - 16..signer_at - 12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(is_encoding(LogEntry::decode(&other)));
}

#[test]
fn account_id_is_stable_and_bound_to_every_byte_of_the_genesis() {
    let a = device(1);
    let kit = recovery(7);
    let log = genesis(&a, &kit);
    let entry = &log.entries()[0];
    let id = account_id_of(entry);

    assert_eq!(id.as_bytes(), &entry.hash()[..16]);
    assert_eq!(account_id_of(entry), id, "deterministic");
    assert_eq!(
        account_id_of(&LogEntry::decode(&entry.encode()).unwrap()),
        id,
        "the stored form gives the same id"
    );
    assert_eq!(
        account_id_of(&genesis(&a, &kit).entries()[0]),
        id,
        "the same Genesis (Ed25519 is deterministic) gives the same id"
    );

    let bytes = entry.encode();
    for i in 0..bytes.len() {
        let mut mutated = bytes.clone();
        mutated[i] ^= 0x01;
        assert_ne!(&kb_crypto::hash(&mutated)[..16], id.as_bytes(), "byte {i}");
        if let Ok(decoded) = LogEntry::decode(&mutated) {
            assert_ne!(account_id_of(&decoded), id, "byte {i}");
        }
    }

    for other in [
        DeviceLog::genesis(a.0, &a.1, kit.verifying_key(), name(1), T0 + 1),
        DeviceLog::genesis(a.0, &a.1, kit.verifying_key(), name(9), T0),
        DeviceLog::genesis(a.0, &a.1, recovery(8).verifying_key(), name(1), T0),
        DeviceLog::genesis(device(2).0, &a.1, kit.verifying_key(), name(1), T0),
    ] {
        assert_ne!(account_id_of(&other.unwrap().entries()[0]), id);
    }
}

#[test]
fn a_forged_signature_is_rejected() {
    let (a, b, c) = (device(1), device(2), device(3));
    let log = genesis(&a, &recovery(7));

    let mut valid = log.clone();
    let entry = add_by(&mut valid, &b, &a, T0 + 1);

    let mut flipped = entry.signature.to_bytes();
    flipped[10] ^= 0x40;
    let mut forged = entry.clone();
    forged.signature = Signature::from_bytes(&flipped).unwrap();
    assert_rejected(&log, &forged, &LogError::Signature { seq: 1 });

    // Content changed after signing.
    let mut edited = entry.clone();
    if let LogEntryBody::DeviceAdded { at, .. } = &mut edited.body {
        *at += 1;
    }
    assert_rejected(&log, &edited, &LogError::Signature { seq: 1 });

    // Claims to be added by A, signed by somebody else.
    let impostor = next_entry(
        &log,
        added_body(&c, Signer::Device(a.0), false),
        b.1.signing(),
    );
    assert_rejected(&log, &impostor, &LogError::Signature { seq: 1 });

    // A Genesis that does not verify under its own device key.
    let bad_genesis = LogEntry::signed(0, [0; 32], log.entries()[0].body.clone(), b.1.signing());
    assert_eq!(
        DeviceLog::from_entries(vec![bad_genesis]).unwrap_err(),
        LogError::Signature { seq: 0 }
    );
}

#[test]
fn reordered_entries_break_the_chain() {
    let (a, b, c) = (device(1), device(2), device(3));
    let mut log = genesis(&a, &recovery(7));
    add_by(&mut log, &b, &a, T0 + 1);
    add_by(&mut log, &c, &a, T0 + 2);
    let e = log.entries().to_vec();

    assert_eq!(
        DeviceLog::from_entries(vec![e[0].clone(), e[2].clone(), e[1].clone()]).unwrap_err(),
        LogError::Chain { seq: 2 }
    );
    assert_eq!(
        DeviceLog::from_entries(vec![e[1].clone(), e[0].clone(), e[2].clone()]).unwrap_err(),
        LogError::Genesis { seq: 1 }
    );
    assert_eq!(
        DeviceLog::from_entries(vec![e[0].clone(), e[2].clone()]).unwrap_err(),
        LogError::Chain { seq: 2 },
        "a missing entry breaks the chain"
    );
    assert_eq!(
        DeviceLog::from_entries(vec![e[0].clone(), e[1].clone(), e[1].clone()]).unwrap_err(),
        LogError::Chain { seq: 1 },
        "a repeated entry breaks the chain"
    );
}

#[test]
fn a_wrong_seq_breaks_the_chain() {
    let (a, b) = (device(1), device(2));
    let log = genesis(&a, &recovery(7));
    let head = log.head_hash().unwrap();
    for seq in [0u64, 2, u64::MAX] {
        let entry = LogEntry::signed(
            seq,
            head,
            added_body(&b, Signer::Device(a.0), false),
            a.1.signing(),
        );
        assert_rejected(&log, &entry, &LogError::Chain { seq });
    }
    // A Genesis with a non-zero seq.
    let genesis_entry = &log.entries()[0];
    let shifted = LogEntry::signed(1, [0; 32], genesis_entry.body.clone(), a.1.signing());
    assert_eq!(
        DeviceLog::from_entries(vec![shifted]).unwrap_err(),
        LogError::Chain { seq: 1 }
    );
}

#[test]
fn a_wrong_prev_hash_breaks_the_chain() {
    let (a, b) = (device(1), device(2));
    let log = genesis(&a, &recovery(7));
    let mut other_head = log.head_hash().unwrap();
    other_head[31] ^= 1;
    for prev in [[0u8; 32], other_head] {
        let entry = LogEntry::signed(
            1,
            prev,
            added_body(&b, Signer::Device(a.0), false),
            a.1.signing(),
        );
        assert_rejected(&log, &entry, &LogError::Chain { seq: 1 });
    }
    // A Genesis whose prev_hash is not zero.
    let genesis_entry = &log.entries()[0];
    let chained = LogEntry::signed(0, other_head, genesis_entry.body.clone(), a.1.signing());
    assert_eq!(
        DeviceLog::from_entries(vec![chained]).unwrap_err(),
        LogError::Chain { seq: 0 }
    );
}

#[test]
fn genesis_comes_first_and_only_first() {
    let (a, b) = (device(1), device(2));
    let kit = recovery(7);

    assert_eq!(
        DeviceLog::from_entries(Vec::new()).unwrap_err(),
        LogError::Empty
    );
    assert_eq!(DeviceLog::default().verify().unwrap_err(), LogError::Empty);

    let not_genesis = LogEntry::signed(
        0,
        [0; 32],
        added_body(&b, Signer::Device(a.0), false),
        a.1.signing(),
    );
    assert_eq!(
        DeviceLog::from_entries(vec![not_genesis.clone()]).unwrap_err(),
        LogError::Genesis { seq: 0 }
    );
    let mut empty = DeviceLog::default();
    assert_eq!(
        empty.append(not_genesis).unwrap_err(),
        LogError::Genesis { seq: 0 }
    );
    assert_eq!(empty, DeviceLog::default());

    let log = genesis(&a, &kit);
    let second = next_entry(
        &log,
        LogEntryBody::Genesis {
            device_id: b.0,
            keys: b.1.public(),
            recovery_key: kit.verifying_key(),
            encrypted_name: name(2),
            at: T0 + 1,
        },
        b.1.signing(),
    );
    assert_rejected(&log, &second, &LogError::Genesis { seq: 1 });
}

#[test]
fn an_unknown_signer_is_rejected() {
    let (a, b, stranger) = (device(1), device(2), device(9));
    let log = genesis(&a, &recovery(7));
    let entry = next_entry(
        &log,
        added_body(&b, Signer::Device(stranger.0), false),
        stranger.1.signing(),
    );
    assert_rejected(&log, &entry, &LogError::InactiveSigner { seq: 1 });

    let revoke = next_entry(
        &log,
        LogEntryBody::DeviceRevoked {
            device_id: a.0,
            revoked_by: stranger.0,
            at: T0,
        },
        stranger.1.signing(),
    );
    assert_rejected(&log, &revoke, &LogError::InactiveSigner { seq: 1 });

    let rotate = next_entry(
        &log,
        LogEntryBody::RecoveryRotated {
            recovery_key: recovery(8).verifying_key(),
            by: stranger.0,
            at: T0,
        },
        stranger.1.signing(),
    );
    assert_rejected(&log, &rotate, &LogError::InactiveSigner { seq: 1 });
}

#[test]
fn a_revoked_signer_is_rejected() {
    let (a, b, c) = (device(1), device(2), device(3));
    let mut log = genesis(&a, &recovery(7));
    add_by(&mut log, &b, &a, T0 + 1);
    log.revoke_device(b.0, (&a.1, a.0), T0 + 2).unwrap();

    let add = next_entry(
        &log,
        added_body(&c, Signer::Device(b.0), false),
        b.1.signing(),
    );
    assert_rejected(&log, &add, &LogError::InactiveSigner { seq: 3 });

    let revoke = next_entry(
        &log,
        LogEntryBody::DeviceRevoked {
            device_id: a.0,
            revoked_by: b.0,
            at: T0 + 3,
        },
        b.1.signing(),
    );
    assert_rejected(&log, &revoke, &LogError::InactiveSigner { seq: 3 });

    let rotate = next_entry(
        &log,
        LogEntryBody::RecoveryRotated {
            recovery_key: recovery(8).verifying_key(),
            by: b.0,
            at: T0 + 3,
        },
        b.1.signing(),
    );
    assert_rejected(&log, &rotate, &LogError::InactiveSigner { seq: 3 });

    // The builders refuse too and leave the log alone.
    let before = log.clone();
    assert_eq!(
        log.add_device(c.0, c.1.public(), name(3), LogSigner::Device(&b.1, b.0), T0)
            .unwrap_err(),
        LogError::InactiveSigner { seq: 3 }
    );
    assert_eq!(
        log.rotate_recovery(recovery(8).verifying_key(), (&b.1, b.0), T0)
            .unwrap_err(),
        LogError::InactiveSigner { seq: 3 }
    );
    assert_eq!(log, before);
}

#[test]
fn a_device_is_added_once_ever() {
    let (a, b) = (device(1), device(2));
    let kit = recovery(7);
    let mut log = genesis(&a, &kit);

    // The Genesis device itself.
    let again = next_entry(
        &log,
        added_body(&a, Signer::Device(a.0), false),
        a.1.signing(),
    );
    assert_rejected(&log, &again, &LogError::DuplicateDevice { seq: 1 });

    add_by(&mut log, &b, &a, T0 + 1);
    let twice = next_entry(
        &log,
        added_body(&b, Signer::Device(a.0), false),
        a.1.signing(),
    );
    assert_rejected(&log, &twice, &LogError::DuplicateDevice { seq: 2 });
    let via_kit = next_entry(&log, added_body(&b, Signer::Recovery, true), &kit);
    assert_rejected(&log, &via_kit, &LogError::DuplicateDevice { seq: 2 });

    // Not even after a revocation.
    log.revoke_device(b.0, (&a.1, a.0), T0 + 2).unwrap();
    let readd = next_entry(
        &log,
        added_body(&b, Signer::Device(a.0), false),
        a.1.signing(),
    );
    assert_rejected(&log, &readd, &LogError::DuplicateDevice { seq: 3 });

    let before = log.clone();
    assert_eq!(
        log.add_device(b.0, b.1.public(), name(2), LogSigner::Device(&a.1, a.0), T0)
            .unwrap_err(),
        LogError::DuplicateDevice { seq: 3 }
    );
    assert_eq!(log, before);
}

#[test]
fn only_an_active_device_can_be_revoked() {
    let (a, b, stranger) = (device(1), device(2), device(9));
    let mut log = genesis(&a, &recovery(7));
    add_by(&mut log, &b, &a, T0 + 1);

    let unknown = next_entry(
        &log,
        LogEntryBody::DeviceRevoked {
            device_id: stranger.0,
            revoked_by: a.0,
            at: T0 + 2,
        },
        a.1.signing(),
    );
    assert_rejected(&log, &unknown, &LogError::NotActive { seq: 2 });

    log.revoke_device(b.0, (&a.1, a.0), T0 + 2).unwrap();
    let twice = next_entry(
        &log,
        LogEntryBody::DeviceRevoked {
            device_id: b.0,
            revoked_by: a.0,
            at: T0 + 3,
        },
        a.1.signing(),
    );
    assert_rejected(&log, &twice, &LogError::NotActive { seq: 3 });

    let before = log.clone();
    assert_eq!(
        log.revoke_device(b.0, (&a.1, a.0), T0 + 3).unwrap_err(),
        LogError::NotActive { seq: 3 }
    );
    assert_eq!(log, before);
    assert_eq!(log.verify().unwrap().devices[1].revoked_at, Some(T0 + 2));
}

#[test]
fn a_device_cannot_revoke_itself() {
    let (a, b) = (device(1), device(2));
    let mut log = genesis(&a, &recovery(7));
    add_by(&mut log, &b, &a, T0 + 1);

    let own = next_entry(
        &log,
        LogEntryBody::DeviceRevoked {
            device_id: b.0,
            revoked_by: b.0,
            at: T0 + 2,
        },
        b.1.signing(),
    );
    assert_rejected(&log, &own, &LogError::InactiveSigner { seq: 2 });

    let before = log.clone();
    assert_eq!(
        log.revoke_device(a.0, (&a.1, a.0), T0 + 2).unwrap_err(),
        LogError::InactiveSigner { seq: 2 }
    );
    assert_eq!(log, before);
}

#[test]
fn kit_signed_additions_follow_the_current_recovery_key() {
    let (a, b, c, d) = (device(1), device(2), device(3), device(4));
    let (kit, kit2, wrong) = (recovery(7), recovery(8), recovery(9));
    let mut log = genesis(&a, &kit);

    // Before a rotation: the Genesis recovery key, and only that one.
    let forged = next_entry(&log, added_body(&b, Signer::Recovery, true), &wrong);
    assert_rejected(&log, &forged, &LogError::Signature { seq: 1 });
    let before = log.clone();
    assert_eq!(
        log.add_device(b.0, b.1.public(), name(2), LogSigner::Recovery(&wrong), T0)
            .unwrap_err(),
        LogError::Signature { seq: 1 }
    );
    assert_eq!(log, before);
    log.add_device(
        b.0,
        b.1.public(),
        name(2),
        LogSigner::Recovery(&kit),
        T0 + 1,
    )
    .unwrap();

    // After a rotation: the old kit no longer signs, the new one does.
    log.rotate_recovery(kit2.verifying_key(), (&b.1, b.0), T0 + 2)
        .unwrap();
    let stale = next_entry(&log, added_body(&c, Signer::Recovery, true), &kit);
    assert_rejected(&log, &stale, &LogError::Signature { seq: 3 });
    log.add_device(
        c.0,
        c.1.public(),
        name(3),
        LogSigner::Recovery(&kit2),
        T0 + 3,
    )
    .unwrap();

    // The rotation applies to the stored form too.
    let state = DeviceLog::from_entries(log.entries().to_vec())
        .unwrap()
        .verify()
        .unwrap();
    assert_eq!(state.recovery_key, kit2.verifying_key());
    assert!(state.devices[1].added_via_kit && state.devices[2].added_via_kit);

    // A device adds as before.
    add_by(&mut log, &d, &c, T0 + 4);
    assert_eq!(log.verify().unwrap().devices.len(), 4);
}

#[test]
fn via_kit_must_match_the_signer() {
    let (a, b) = (device(1), device(2));
    let kit = recovery(7);
    let log = genesis(&a, &kit);

    let unflagged = next_entry(&log, added_body(&b, Signer::Recovery, false), &kit);
    let flagged = next_entry(
        &log,
        added_body(&b, Signer::Device(a.0), true),
        a.1.signing(),
    );
    for entry in [unflagged, flagged] {
        let mut copy = log.clone();
        assert!(matches!(copy.append(entry), Err(LogError::Encoding(_))));
        assert_eq!(copy, log);
    }
}

#[test]
fn recovery_rotation_is_signed_by_an_active_device() {
    let a = device(1);
    let kit = recovery(7);
    let log = genesis(&a, &kit);
    // Signed by the recovery key while naming a device.
    let entry = next_entry(
        &log,
        LogEntryBody::RecoveryRotated {
            recovery_key: recovery(8).verifying_key(),
            by: a.0,
            at: T0 + 1,
        },
        &kit,
    );
    assert_rejected(&log, &entry, &LogError::Signature { seq: 1 });
}

#[test]
fn an_unsupported_version_is_rejected() {
    let (a, b) = (device(1), device(2));
    let log = genesis(&a, &recovery(7));
    let mut entry = next_entry(
        &log,
        added_body(&b, Signer::Device(a.0), false),
        a.1.signing(),
    );
    entry.version = 2;
    entry.signature = kb_crypto::sign(a.1.signing(), LOG_CONTEXT, &entry.signed_bytes());
    let mut copy = log.clone();
    assert!(matches!(copy.append(entry), Err(LogError::Encoding(_))));
    assert_eq!(copy, log);
}

#[test]
fn a_builder_signing_with_other_keys_than_the_logged_ones_fails() {
    let (a, b, c) = (device(1), device(2), device(3));
    let mut log = genesis(&a, &recovery(7));
    let before = log.clone();
    // C's keys presented as A.
    assert_eq!(
        log.add_device(b.0, b.1.public(), name(2), LogSigner::Device(&c.1, a.0), T0)
            .unwrap_err(),
        LogError::Signature { seq: 1 }
    );
    assert_eq!(log, before);
}

#[test]
fn builders_need_a_genesis() {
    let (a, b) = (device(1), device(2));
    let mut log = DeviceLog::default();
    assert_eq!(
        log.add_device(b.0, b.1.public(), name(2), LogSigner::Device(&a.1, a.0), T0)
            .unwrap_err(),
        LogError::Empty
    );
    assert_eq!(
        log.revoke_device(b.0, (&a.1, a.0), T0).unwrap_err(),
        LogError::Empty
    );
    assert_eq!(
        log.rotate_recovery(recovery(8).verifying_key(), (&a.1, a.0), T0)
            .unwrap_err(),
        LogError::Empty
    );
    assert_eq!(
        log.rotate_root(1, Vec::new(), (&a.1, a.0), T0).unwrap_err(),
        LogError::Empty
    );
    assert_eq!(log, DeviceLog::default());
}

#[test]
fn append_takes_entries_made_by_another_copy() {
    let s = story();
    let mut replica = DeviceLog::default();
    for entry in s.log.entries() {
        replica.append(entry.clone()).unwrap();
    }
    assert_eq!(replica, s.log);
    assert_eq!(replica.verify().unwrap(), s.log.verify().unwrap());

    // A copy that is behind takes the next entry made elsewhere.
    let mut ahead = s.log.clone();
    let e = device(5);
    let entry = add_by(&mut ahead, &e, &s.d, T0 + 9);
    let mut behind = s.log.clone();
    behind.append(entry).unwrap();
    assert_eq!(behind, ahead);

    // A log that does not verify accepts nothing.
    let mut broken_entries = s.log.entries().to_vec();
    broken_entries[1].signature = broken_entries[2].signature;
    let mut broken = DeviceLog {
        entries: broken_entries,
    };
    let next = next_entry(
        &broken,
        added_body(&device(6), Signer::Device(s.a.0), false),
        s.a.1.signing(),
    );
    assert_eq!(
        broken.append(next).unwrap_err(),
        LogError::Signature { seq: 1 }
    );
}

#[test]
fn log_errors_name_sequence_numbers_only() {
    assert_eq!(LogError::Empty.to_string(), "the device log is empty");
    assert_eq!(
        LogError::Chain { seq: 3 }.to_string(),
        "entry 3 breaks the chain"
    );
    assert_eq!(
        LogError::Genesis { seq: 1 }.to_string(),
        "entry 1 is not where a Genesis belongs"
    );
    assert_eq!(
        LogError::Signature { seq: 2 }.to_string(),
        "entry 2 has a bad signature"
    );
    assert_eq!(
        LogError::InactiveSigner { seq: 4 }.to_string(),
        "entry 4 is signed by a device that is not active"
    );
    assert_eq!(
        LogError::DuplicateDevice { seq: 5 }.to_string(),
        "entry 5 adds a device that already exists or names one twice"
    );
    assert_eq!(
        LogError::NotActive { seq: 6 }.to_string(),
        "entry 6 names a device that is not active"
    );
    assert_eq!(
        LogError::IncompleteRotation { seq: 7 }.to_string(),
        "entry 7 does not seal the root key to every active device"
    );
}

#[test]
fn device_names_round_trip_under_the_ark() {
    let ark = ark(1);
    let (a, _) = device(1);
    let rng = SeededRng::new(3);
    for text in ["Ben's laptop", "", "Pixel 9 \u{1F4F1} \u{00E9}t\u{00E9}"] {
        let sealed = encrypt_name(&ark, a, text, &rng);
        assert_eq!(sealed.len(), text.len() + NONCE_LEN + TAG_LEN);
        assert_eq!(decrypt_name(&ark, a, &sealed).unwrap(), text);
    }
    let one = encrypt_name(&ark, a, "same", &rng);
    let two = encrypt_name(&ark, a, "same", &rng);
    assert_ne!(one, two, "fresh nonce every time");
}

#[test]
fn device_names_use_the_documented_construction() {
    let ark = ark(1);
    let (a, _) = device(1);
    let sealed = encrypt_name(&ark, a, "Desk", &SeededRng::new(3));
    let key = kb_crypto::derive_key(ark.secret(), Label::DEVICE_NAME, a.as_bytes());
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let plaintext = kb_crypto::aead_open(
        &key,
        &Nonce24::from_bytes(nonce.try_into().unwrap()),
        a.as_bytes(),
        ciphertext,
    )
    .unwrap();
    assert_eq!(plaintext.as_slice(), b"Desk");
}

#[test]
fn device_names_are_bound_to_the_ark_and_the_device() {
    let ark1 = ark(1);
    let (a, _) = device(1);
    let (b, _) = device(2);
    let sealed = encrypt_name(&ark1, a, "Ben's laptop", &SeededRng::new(3));
    let auth = |r: Result<String, KeysError>| {
        matches!(r, Err(KeysError::Crypto(CryptoError::Authentication)))
    };

    assert!(auth(decrypt_name(&ark1, b, &sealed)), "other device id");
    assert!(auth(decrypt_name(&ark(2), a, &sealed)), "other ARK");
    for i in 0..sealed.len() {
        let mut tampered = sealed.clone();
        tampered[i] ^= 0x80;
        assert!(auth(decrypt_name(&ark1, a, &tampered)), "byte {i}");
    }
    for len in 0..NONCE_LEN + TAG_LEN {
        let err = decrypt_name(&ark1, a, &sealed[..len]).unwrap_err();
        assert!(
            matches!(err, KeysError::Crypto(CryptoError::Encoding(_))),
            "{len}: {err}"
        );
    }
    let err = decrypt_name(&ark1, b, &sealed).unwrap_err().to_string();
    assert!(!err.contains("Ben"), "{err}");
}

#[test]
fn a_name_that_is_not_utf8_is_corrupted() {
    let ark = ark(1);
    let (a, _) = device(1);
    let key = kb_crypto::derive_key(ark.secret(), Label::DEVICE_NAME, a.as_bytes());
    let nonce = Nonce24::from_bytes([4; NONCE_LEN]);
    let mut sealed = nonce.as_bytes().to_vec();
    sealed.extend(kb_crypto::aead_seal(
        &key,
        &nonce,
        a.as_bytes(),
        &[0xFF, 0xFE],
    ));
    assert!(matches!(
        decrypt_name(&ark, a, &sealed),
        Err(KeysError::Corrupted(_))
    ));
}

// ---------------------------------------------------------------------------------
// Root key rotation (ADR-0022, tag 5)

/// Stand-in for a sealed root key box (the log treats boxes as opaque bytes).
fn sealed_box(tag: u8) -> Vec<u8> {
    vec![tag; 20]
}

/// Genesis(A); A adds B, C and D; A revokes D; A rotates the root key to generation
/// 1 (boxes for B and C); B rotates it to generation 2 (boxes for A and C).
struct Rotation {
    a: Device,
    b: Device,
    c: Device,
    d: Device,
    log: DeviceLog,
}

fn rotation_story() -> Rotation {
    let (a, b, c, d) = (device(1), device(2), device(3), device(4));
    let mut log = genesis(&a, &recovery(7));
    add_by(&mut log, &b, &a, T0 + 1);
    add_by(&mut log, &c, &a, T0 + 2);
    add_by(&mut log, &d, &a, T0 + 3);
    log.revoke_device(d.0, (&a.1, a.0), T0 + 4).unwrap();
    log.rotate_root(
        1,
        vec![(b.0, sealed_box(1)), (c.0, sealed_box(2))],
        (&a.1, a.0),
        T0 + 5,
    )
    .unwrap();
    log.rotate_root(
        2,
        vec![(a.0, sealed_box(3)), (c.0, sealed_box(4))],
        (&b.1, b.0),
        T0 + 6,
    )
    .unwrap();
    Rotation { a, b, c, d, log }
}

fn rotated_body(generation: u32, boxes: Vec<(DeviceId, Vec<u8>)>, by: DeviceId) -> LogEntryBody {
    LogEntryBody::RootRotated {
        generation,
        boxes,
        by,
        at: T0 + 50,
    }
}

#[test]
fn root_rotations_advance_the_generation_and_the_state_records_who_did_what() {
    let r = rotation_story();
    let entries = r.log.entries();
    assert_eq!(entries.len(), 7);
    for (len, generation) in [(1, 0), (5, 0), (6, 1), (7, 2)] {
        let prefix = DeviceLog::from_entries(entries[..len].to_vec()).unwrap();
        assert_eq!(
            prefix.verify().unwrap().root_generation,
            generation,
            "{len}"
        );
    }
    assert!(matches!(
        &entries[6].body,
        LogEntryBody::RootRotated { generation: 2, by, boxes, at }
            if *by == r.b.0 && *at == T0 + 6 && boxes.len() == 2
    ));

    let state = r.log.verify().unwrap();
    assert_eq!(state.root_generation, 2);
    assert_eq!(state.revoked_at(r.d.0), Some(T0 + 4));
    assert_eq!(state.revoked_at(r.a.0), None);
    assert_eq!(state.revoked_at(device(9).0), None, "unknown device");
    let by_id = |id: DeviceId| state.devices.iter().find(|d| d.device_id == id).unwrap();
    assert_eq!(by_id(r.d.0).revoked_by, Some(r.a.0));
    assert_eq!(by_id(r.c.0).revoked_by, None);
    assert_eq!(
        by_id(r.a.0).added_by,
        Signer::Device(r.a.0),
        "the Genesis device"
    );
    assert_eq!(by_id(r.b.0).added_by, Signer::Device(r.a.0));

    // The stored form rebuilds the same log and state.
    let stored: Vec<LogEntry> = entries
        .iter()
        .map(|e| LogEntry::decode(&e.encode()).unwrap())
        .collect();
    let rebuilt = DeviceLog::from_entries(stored).unwrap();
    assert_eq!(rebuilt, r.log);
    assert_eq!(rebuilt.verify().unwrap(), state);

    // A replica takes the rotations one by one.
    let mut replica = DeviceLog::from_entries(entries[..5].to_vec()).unwrap();
    replica.append(entries[5].clone()).unwrap();
    replica.append(entries[6].clone()).unwrap();
    assert_eq!(replica, r.log);
}

#[test]
fn a_root_generation_must_follow_the_current_one() {
    let r = rotation_story();
    let log = &r.log;
    for generation in [0u32, 1, 2, 4, 100, u32::MAX] {
        let entry = next_entry(
            log,
            rotated_body(generation, vec![(r.b.0, sealed_box(5))], r.a.0),
            r.a.1.signing(),
        );
        assert_rejected(
            log,
            &entry,
            &LogError::Encoding(format!(
                "entry 7: root generation {generation} does not follow 2"
            )),
        );
    }
    let full = || vec![(r.b.0, sealed_box(5)), (r.c.0, sealed_box(6))];
    let mut copy = log.clone();
    assert!(matches!(
        copy.rotate_root(4, full(), (&r.a.1, r.a.0), T0 + 9),
        Err(LogError::Encoding(_))
    ));
    assert_eq!(&copy, log);
    copy.rotate_root(3, full(), (&r.a.1, r.a.0), T0 + 9)
        .unwrap();
    assert_eq!(copy.verify().unwrap().root_generation, 3);
}

#[test]
fn the_last_generation_does_not_wrap_around() {
    let r = rotation_story();
    let mut state = r.log.verify().unwrap();
    state.root_generation = u32::MAX;
    let mut replay = Replay {
        state: Some(state),
        head: r.log.head_hash().unwrap(),
        next_seq: 7,
    };
    let boxes = vec![(r.b.0, sealed_box(5)), (r.c.0, sealed_box(6))];
    let wrapped = next_entry(&r.log, rotated_body(0, boxes, r.a.0), r.a.1.signing());
    assert!(matches!(replay.apply(&wrapped), Err(LogError::Encoding(_))));
    assert_eq!(replay.next_seq, 7, "nothing applied");
}

#[test]
fn root_key_boxes_go_to_active_devices_other_than_the_signer_once_each() {
    let r = rotation_story();
    let log = &r.log;
    let stranger = device(9);
    let a_signs = |boxes: Vec<(DeviceId, Vec<u8>)>| {
        next_entry(log, rotated_body(3, boxes, r.a.0), r.a.1.signing())
    };

    let not_active = LogError::NotActive { seq: 7 };
    for boxes in [
        vec![(r.d.0, sealed_box(5))],
        vec![(stranger.0, sealed_box(5))],
        vec![(r.a.0, sealed_box(5))],
        vec![(r.b.0, sealed_box(5)), (r.d.0, sealed_box(6))],
    ] {
        assert_rejected(log, &a_signs(boxes), &not_active);
    }
    assert_rejected(
        log,
        &a_signs(vec![
            (r.b.0, sealed_box(5)),
            (r.c.0, sealed_box(6)),
            (r.b.0, sealed_box(7)),
        ]),
        &LogError::DuplicateDevice { seq: 7 },
    );

    // The builder refuses too and leaves the log alone.
    let mut copy = log.clone();
    assert_eq!(
        copy.rotate_root(3, vec![(r.d.0, sealed_box(5))], (&r.a.1, r.a.0), T0)
            .unwrap_err(),
        not_active
    );
    assert_eq!(&copy, log);

    // Every active device but the signer, in any order, is what verifies.
    for boxes in [
        vec![(r.b.0, sealed_box(5)), (r.c.0, sealed_box(6))],
        vec![(r.c.0, sealed_box(6)), (r.b.0, sealed_box(5))],
    ] {
        let mut copy = log.clone();
        copy.append(a_signs(boxes)).unwrap();
        assert_eq!(copy.verify().unwrap().root_generation, 3);
    }
}

#[test]
fn a_root_rotation_seals_to_every_other_active_device() {
    let r = rotation_story();
    let log = &r.log;
    let incomplete = LogError::IncompleteRotation { seq: 7 };
    // Active: A, B and C (D was revoked). A rotates: B and C must both be boxed.
    for boxes in [
        Vec::new(),
        vec![(r.b.0, sealed_box(5))],
        vec![(r.c.0, sealed_box(6))],
    ] {
        let entry = next_entry(log, rotated_body(3, boxes.clone(), r.a.0), r.a.1.signing());
        assert_rejected(log, &entry, &incomplete);
        let mut copy = log.clone();
        assert_eq!(
            copy.rotate_root(3, boxes, (&r.a.1, r.a.0), T0).unwrap_err(),
            incomplete
        );
        assert_eq!(&copy, log);
    }
    // An extra device is refused whether or not every active one is there.
    for boxes in [
        vec![
            (r.b.0, sealed_box(5)),
            (r.c.0, sealed_box(6)),
            (r.d.0, sealed_box(7)),
        ],
        vec![
            (r.b.0, sealed_box(5)),
            (r.c.0, sealed_box(6)),
            (r.a.0, sealed_box(7)),
        ],
    ] {
        let entry = next_entry(log, rotated_body(3, boxes, r.a.0), r.a.1.signing());
        assert_rejected(log, &entry, &LogError::NotActive { seq: 7 });
    }

    // A device added after a rotation must be boxed by the next one; a device
    // revoked in between must not.
    let e = device(5);
    let mut grown = log.clone();
    add_by(&mut grown, &e, &r.c, T0 + 10);
    let stale = next_entry(
        &grown,
        rotated_body(
            3,
            vec![(r.b.0, sealed_box(5)), (r.c.0, sealed_box(6))],
            r.a.0,
        ),
        r.a.1.signing(),
    );
    assert_rejected(&grown, &stale, &LogError::IncompleteRotation { seq: 8 });
    grown
        .revoke_device(r.b.0, (&r.a.1, r.a.0), T0 + 11)
        .unwrap();
    grown
        .rotate_root(
            3,
            vec![(e.0, sealed_box(8)), (r.c.0, sealed_box(9))],
            (&r.a.1, r.a.0),
            T0 + 12,
        )
        .unwrap();
    assert_eq!(grown.verify().unwrap().root_generation, 3);

    // The only active device rotates with no box at all.
    let mut alone = genesis(&r.a, &recovery(7));
    alone
        .rotate_root(1, Vec::new(), (&r.a.1, r.a.0), T0 + 1)
        .unwrap();
    assert_eq!(alone.verify().unwrap().root_generation, 1);
}

#[test]
fn a_root_rotation_is_signed_by_an_active_device() {
    let r = rotation_story();
    let log = &r.log;
    let stranger = device(9);
    let boxes = || vec![(r.c.0, sealed_box(5))];

    let by_revoked = next_entry(log, rotated_body(3, boxes(), r.d.0), r.d.1.signing());
    assert_rejected(log, &by_revoked, &LogError::InactiveSigner { seq: 7 });
    let by_stranger = next_entry(
        log,
        rotated_body(3, boxes(), stranger.0),
        stranger.1.signing(),
    );
    assert_rejected(log, &by_stranger, &LogError::InactiveSigner { seq: 7 });
    let impostor = next_entry(log, rotated_body(3, boxes(), r.a.0), r.b.1.signing());
    assert_rejected(log, &impostor, &LogError::Signature { seq: 7 });
    let by_kit = next_entry(log, rotated_body(3, boxes(), r.a.0), &recovery(7));
    assert_rejected(log, &by_kit, &LogError::Signature { seq: 7 });

    let mut copy = log.clone();
    assert_eq!(
        copy.rotate_root(3, boxes(), (&r.d.1, r.d.0), T0)
            .unwrap_err(),
        LogError::InactiveSigner { seq: 7 }
    );
    assert_eq!(&copy, log);
}

#[test]
fn a_root_rotation_seals_at_most_max_root_boxes() {
    let r = rotation_story();
    let log = &r.log;
    let too_many = vec![(r.c.0, Vec::new()); MAX_ROOT_BOXES + 1];
    let mut copy = log.clone();
    assert!(matches!(
        copy.rotate_root(3, too_many.clone(), (&r.a.1, r.a.0), T0),
        Err(LogError::Encoding(_))
    ));
    assert_eq!(&copy, log);

    // The decoder refuses such a count before reading the boxes.
    let entry = next_entry(log, rotated_body(3, too_many, r.a.0), r.a.1.signing());
    assert_eq!(
        LogEntry::decode(&entry.encode()).unwrap_err(),
        LogError::Encoding("a root rotation names 1025 devices".to_owned())
    );
    let mut bytes = entry.encode();
    bytes[47..51].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        LogEntry::decode(&bytes),
        Err(LogError::Encoding(_))
    ));

    // At the bound the count is fine (here the duplicates are what fails).
    let at_bound = next_entry(
        log,
        rotated_body(3, vec![(r.c.0, Vec::new()); MAX_ROOT_BOXES], r.a.0),
        r.a.1.signing(),
    );
    assert_rejected(log, &at_bound, &LogError::DuplicateDevice { seq: 7 });
}

#[test]
fn root_rotation_entries_decode_strictly() {
    let r = rotation_story();
    let is_encoding = |r: Result<LogEntry, LogError>| matches!(r, Err(LogError::Encoding(_)));
    for entry in &r.log.entries()[5..] {
        let bytes = entry.encode();
        assert_eq!(bytes[42], 5, "tag");
        for len in 0..bytes.len() {
            assert!(
                is_encoding(LogEntry::decode(&bytes[..len])),
                "truncated to {len}"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(is_encoding(LogEntry::decode(&longer)), "trailing byte");
        // A box length pointing past the end.
        let mut other = bytes.clone();
        other[51 + 16..51 + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(is_encoding(LogEntry::decode(&other)));
    }
}

#[test]
fn device_keys_cover_every_device_ever_added() {
    let s = story();
    for d in [&s.a, &s.b, &s.c, &s.d] {
        assert_eq!(s.log.device_keys(d.0), Some(d.1.public()));
    }
    // A revoked device (B) is still found: entries it signed before its revocation stay valid.
    assert!(s.log.verify().unwrap().active(s.b.0).is_none());
    let stranger = device(99);
    assert_eq!(s.log.device_keys(stranger.0), None);
    assert_eq!(DeviceLog::default().device_keys(s.a.0), None);
}

#[test]
fn entry_hash_names_each_position() {
    let s = story();
    for (seq, entry) in s.log.entries().iter().enumerate() {
        assert_eq!(
            s.log.entry_hash(u64::try_from(seq).unwrap()),
            Some(entry.hash())
        );
    }
    assert_eq!(s.log.entry_hash(6), None);
    assert_eq!(s.log.entry_hash(u64::MAX), None);
}

#[test]
fn active_devices_follow_the_log_entry_by_entry() {
    let s = story();
    let ids = |seq: u64| -> Vec<DeviceId> {
        s.log
            .active_devices_at(seq)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    };
    // Genesis(A); A adds B; the kit adds C; C revokes B; rotate kit; the kit adds D.
    assert_eq!(ids(0), [s.a.0]);
    assert_eq!(ids(1), [s.a.0, s.b.0]);
    assert_eq!(ids(2), [s.a.0, s.b.0, s.c.0]);
    assert_eq!(ids(3), [s.a.0, s.c.0], "B revoked");
    assert_eq!(
        ids(4),
        [s.a.0, s.c.0],
        "a recovery rotation changes no device"
    );
    assert_eq!(ids(5), [s.a.0, s.c.0, s.d.0]);
    assert_eq!(s.log.active_devices_at(6), None);
    assert_eq!(s.log.active_devices_at(u64::MAX), None);
    // The keys come with the ids.
    let (id, keys) = s.log.active_devices_at(5).unwrap()[2];
    assert_eq!((id, keys), (s.d.0, s.d.1.public()));
    // The last position is the state the log verifies to.
    let state = s.log.verify().unwrap();
    let verified: Vec<DeviceId> = state
        .devices
        .iter()
        .filter(|d| d.revoked_at.is_none())
        .map(|d| d.device_id)
        .collect();
    assert_eq!(ids(5), verified);
}
