//! The golden files of the key-management formats (`fixtures/keys/*-v1.bin`, written
//! by `examples/make_keys_vectors.rs`): each one decodes, verifies and opens with
//! today's code, and regenerating it from its fixed inputs gives the same bytes. A
//! failure here means a stored or transmitted format changed: that needs a version
//! bump, not a regenerated file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/vectors.rs"]
mod vectors;

use kb_keys::log::{DeviceLog, LogEntryBody, Signer, decrypt_name};
use kb_keys::pairing::{Bundle, Identity, MessageKind};
use kb_keys::recovery::RecoveryBlob;
use kb_keys::rotation::open_root_box;

const DEVICE_LOG: &[u8] = include_bytes!("../../../fixtures/keys/device-log-v1.bin");
const RECOVERY_BLOB: &[u8] = include_bytes!("../../../fixtures/keys/recovery-blob-v1.bin");
const OFFER: &[u8] = include_bytes!("../../../fixtures/keys/pairing-offer-v1.bin");
const ANSWER: &[u8] = include_bytes!("../../../fixtures/keys/pairing-answer-v1.bin");
const REVEAL: &[u8] = include_bytes!("../../../fixtures/keys/pairing-reveal-v1.bin");
const IDENTITY: &[u8] = include_bytes!("../../../fixtures/keys/pairing-identity-v1.bin");
const BUNDLE: &[u8] = include_bytes!("../../../fixtures/keys/pairing-bundle-v1.bin");

fn frozen(name: &str) -> &'static [u8] {
    match name {
        "device-log-v1.bin" => DEVICE_LOG,
        "recovery-blob-v1.bin" => RECOVERY_BLOB,
        "pairing-offer-v1.bin" => OFFER,
        "pairing-answer-v1.bin" => ANSWER,
        "pairing-reveal-v1.bin" => REVEAL,
        "pairing-identity-v1.bin" => IDENTITY,
        "pairing-bundle-v1.bin" => BUNDLE,
        other => panic!("unknown golden {other}"),
    }
}

#[test]
fn regenerating_gives_the_frozen_bytes() {
    for (name, bytes) in vectors::generate() {
        assert_eq!(bytes, frozen(name), "{name} changed");
    }
}

#[test]
fn the_device_log_golden_verifies_with_every_entry_kind() {
    let entries = vectors::decode_log(DEVICE_LOG);
    let tags: Vec<u8> = entries.iter().map(|e| e.encode()[42]).collect();
    assert_eq!(
        tags,
        [1, 2, 2, 3, 4, 5, 2],
        "every body tag, in story order"
    );

    let log = DeviceLog::from_entries(entries).unwrap();
    let state = log.verify().unwrap();
    let (a, b, c, d) = (
        vectors::device(1),
        vectors::device(2),
        vectors::device(3),
        vectors::device(4),
    );
    assert_eq!(state.root_generation, 1);
    assert_eq!(
        state.recovery_key,
        vectors::kit(vectors::KIT_1).verifying_key()
    );
    assert_eq!(state.revoked_at(b.0), Some(vectors::T0 + 3));
    assert!(state.active(a.0).is_some() && state.active(c.0).is_some());
    let ids: Vec<_> = state.devices.iter().map(|r| r.device_id).collect();
    assert_eq!(ids, [a.0, b.0, c.0, d.0]);
    assert_eq!(state.devices[2].added_by, Signer::Recovery);
    assert!(state.devices[2].added_via_kit);
    assert_eq!(state.devices[3].added_by, Signer::Device(c.0));

    // Names: the early ones under generation 0, the joiner's under generation 1.
    let ark_0 = vectors::ark(vectors::ARK_0);
    let ark_1 = vectors::ark(vectors::ARK_1);
    let names: Vec<String> = state.devices[..3]
        .iter()
        .map(|r| decrypt_name(&ark_0, r.device_id, &r.encrypted_name).unwrap())
        .collect();
    assert_eq!(names, ["Desk", "Laptop", "Phone"]);
    assert_eq!(
        decrypt_name(&ark_1, d.0, &state.devices[3].encrypted_name).unwrap(),
        "Tablet"
    );

    // C opens its box of the rotation; A (the signer) has none.
    let LogEntryBody::RootRotated {
        generation,
        boxes,
        by,
        ..
    } = &log.entries()[5].body
    else {
        panic!("entry 5 is the rotation");
    };
    assert_eq!((*generation, *by), (1, a.0));
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].0, c.0);
    let opened = open_root_box(&c.1, 1, &boxes[0].1).unwrap();
    let expected = vectors::rotation_box();
    assert_eq!(opened.ark, expected.ark);
    assert_eq!(opened.ark, ark_1);
    assert_eq!(opened.recovery_id, expected.recovery_id);
    assert_eq!(opened.recovery_blob, RECOVERY_BLOB);
    assert!(open_root_box(&a.1, 1, &boxes[0].1).is_err());
}

#[test]
fn the_recovery_blob_golden_opens_with_the_new_kit_only() {
    let blob = RecoveryBlob::decode(RECOVERY_BLOB).unwrap();
    assert_eq!(blob.generation, 1);
    assert_eq!(blob.account_id, vectors::account_id());
    assert_eq!(RECOVERY_BLOB[0], 1, "blob version");
    let ark = vectors::kit(vectors::KIT_1)
        .unwrap_ark(&blob.wrapped)
        .unwrap();
    assert_eq!(ark, vectors::ark(vectors::ARK_1));
    assert!(
        vectors::kit(vectors::KIT_0)
            .unwrap_ark(&blob.wrapped)
            .is_err()
    );
}

#[test]
fn the_pairing_goldens_agree_and_open() {
    let (joiner, sponsor, offer, answer, reveal) = vectors::sessions();
    assert_eq!(offer, OFFER);
    assert_eq!(answer, ANSWER);
    assert_eq!(reveal, REVEAL);
    assert_eq!(&OFFER[..2], &[1, 0], "version 1, joiner");
    assert_eq!(&ANSWER[..2], &[1, 1], "version 1, sponsor");
    assert_eq!(&REVEAL[..2], &[1, 0], "version 1, joiner");
    // The offer commits to the key the reveal carries.
    let mut committed = b"atomik-kb/v1/pairing-commit".to_vec();
    committed.extend_from_slice(&REVEAL[2..]);
    assert_eq!(&OFFER[2..], kb_crypto::hash(&committed).as_slice());
    assert_eq!(joiner.sas().as_deref(), Some(vectors::SAS));
    assert_eq!(sponsor.sas().as_deref(), Some(vectors::SAS));

    let (kind, plaintext) = sponsor.open(IDENTITY, vectors::SEQ_IDENTITY).unwrap();
    assert_eq!(kind, MessageKind::Identity);
    assert_eq!(Identity::decode(&plaintext).unwrap(), vectors::identity());
    assert_eq!(
        &IDENTITY[..3],
        &[1, 1, 6],
        "envelope v1, suite 1, kind Pairing"
    );

    let (kind, plaintext) = joiner.open(BUNDLE, vectors::SEQ_BUNDLE).unwrap();
    assert_eq!(kind, MessageKind::Bundle);
    let bundle = Bundle::decode(&plaintext).unwrap();
    let expected = vectors::bundle();
    assert_eq!(bundle.ark, expected.ark);
    assert_eq!(bundle.ark_generation, 1);
    assert_eq!(bundle.account_id, vectors::account_id());
    assert_eq!(bundle.server_url, vectors::SERVER);
    assert_eq!(bundle.recovery_id, expected.recovery_id);
    assert_eq!(bundle.recovery_blob, RECOVERY_BLOB);
    assert_eq!(bundle.workspace_id, expected.workspace_id);
    assert_eq!(bundle.workspace_name, "Notes");
    // The bundle's log is the golden log, and it verifies on the joiner.
    let entries: Vec<_> = bundle
        .log
        .iter()
        .map(|bytes| kb_keys::log::LogEntry::decode(bytes).unwrap())
        .collect();
    assert_eq!(entries, vectors::decode_log(DEVICE_LOG));
    DeviceLog::from_entries(entries).unwrap();

    // Each side refuses its own message, and another sequence number.
    assert!(sponsor.open(IDENTITY, 0).is_err());
    assert!(joiner.open(BUNDLE, vectors::SEQ_IDENTITY).is_err());
    assert!(joiner.open(IDENTITY, vectors::SEQ_IDENTITY).is_err());
    assert!(sponsor.open(BUNDLE, vectors::SEQ_BUNDLE).is_err());
}
