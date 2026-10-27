//! The fixed inputs behind the key-management golden files in `fixtures/keys/`
//! (ADR-0017 device log, ADR-0022 pairing, root rotation and recovery blob).
//!
//! Shared by `examples/make_keys_vectors.rs` (writes the files) and `tests/goldens.rs`
//! (checks them on every test run). Keys are fixed bytes, every random byte (ephemeral
//! keys, nonces) comes from `SeededRng` with the seed named here, and Ed25519
//! signatures are deterministic, so the output is reproducible byte for byte.

#![allow(dead_code)]

use kb_crypto::Secret32;
use kb_keys::log::{DeviceLog, LogEntry, LogSigner, encrypt_name};
use kb_keys::pairing::{Bundle, Identity, MessageKind, Session};
use kb_keys::recovery::RecoveryBlob;
use kb_keys::rotation::{RotationBox, seal_root_for};
use kb_keys::{Ark, DeviceKeys, RecoveryKey};
use kb_types::testing::SeededRng;
use kb_types::{AccountId, DeviceId, WorkspaceId};

/// The files, in the order the example writes them.
pub const FILES: [&str; 7] = [
    "device-log-v1.bin",
    "recovery-blob-v1.bin",
    "pairing-offer-v1.bin",
    "pairing-answer-v1.bin",
    "pairing-reveal-v1.bin",
    "pairing-identity-v1.bin",
    "pairing-bundle-v1.bin",
];

/// Mailbox sequence numbers of the sealed identity and bundle (0 offer, 1 answer,
/// 2 reveal).
pub const SEQ_IDENTITY: u32 = 3;
pub const SEQ_BUNDLE: u32 = 4;

/// The root key of generation 0 and of generation 1 (after the rotation).
pub const ARK_0: [u8; 32] = [0x20; 32];
pub const ARK_1: [u8; 32] = [0x21; 32];
/// The first recovery kit and the one issued with the rotation.
pub const KIT_0: [u8; 32] = [0x30; 32];
pub const KIT_1: [u8; 32] = [0x31; 32];
/// Fixed time of the log entries (Unix ms, 2026-10-05).
pub const T0: i64 = 1_791_158_400_000;
/// The pairing mailbox and server.
pub const MAILBOX: [u8; 6] = [1, 2, 3, 4, 5, 6];
pub const SERVER: &str = "https://sync.example.com";
/// Seeds of the generators: device names, the root key box, the recovery blob's wrap,
/// the joiner's and the sponsor's ephemeral keys, and the two sealed messages.
pub const NAME_SEED: u64 = 0x7_0001;
pub const BOX_SEED: u64 = 0x7_0002;
pub const WRAP_SEED: u64 = 0x7_0003;
pub const JOINER_SEED: u64 = 1;
pub const SPONSOR_SEED: u64 = 2;
pub const IDENTITY_SEED: u64 = 0x7_0004;
pub const BUNDLE_SEED: u64 = 0x7_0005;
/// The digits both sides show for `JOINER_SEED`, `SPONSOR_SEED` and `MAILBOX`
/// (cross-checked with Python's `cryptography`, see `src/pairing/tests.rs`).
pub const SAS: &str = "867987";

pub fn ark(bytes: [u8; 32]) -> Ark {
    Ark::from_secret(Secret32::from_bytes(bytes))
}

pub fn kit(bytes: [u8; 32]) -> RecoveryKey {
    RecoveryKey::from_secret(Secret32::from_bytes(bytes))
}

/// Device `n` (1 = A, 2 = B, 3 = C, 4 = the joiner D): fixed id and seeds.
pub fn device(n: u8) -> (DeviceId, DeviceKeys) {
    (
        DeviceId::from_random_bytes([0xd0 | n; 16]),
        DeviceKeys::from_seeds(
            &Secret32::from_bytes([0x40 | n; 32]),
            &Secret32::from_bytes([0x50 | n; 32]),
        ),
    )
}

/// What device C finds in its box of the `RootRotated` entry.
pub fn rotation_box() -> RotationBox {
    RotationBox {
        ark: ark(ARK_1),
        recovery_id: kit(KIT_1).recovery_id(),
        recovery_blob: recovery_blob(),
    }
}

/// The Genesis by A with kit 0; A's name takes the first nonce of `names`.
fn genesis(names: &SeededRng) -> DeviceLog {
    let a = device(1);
    DeviceLog::genesis(
        a.0,
        &a.1,
        kit(KIT_0).verifying_key(),
        encrypt_name(&ark(ARK_0), a.0, "Desk", names),
        T0,
    )
    .unwrap()
}

/// Every entry kind and both signer kinds: Genesis(A, kit 0); A adds B; kit 0 adds C;
/// A revokes B; A rotates the kit to kit 1; A rotates the root key to generation 1
/// with a box for C; C adds D (the joiner).
pub fn device_log() -> DeviceLog {
    let names = SeededRng::new(NAME_SEED);
    let (a, b, c, d) = (device(1), device(2), device(3), device(4));
    let root = ark(ARK_0);
    let name =
        |device: &(DeviceId, DeviceKeys), text: &str| encrypt_name(&root, device.0, text, &names);
    let mut log = genesis(&names);
    log.add_device(
        b.0,
        b.1.public(),
        name(&b, "Laptop"),
        LogSigner::Device(&a.1, a.0),
        T0 + 1,
    )
    .unwrap();
    log.add_device(
        c.0,
        c.1.public(),
        name(&c, "Phone"),
        LogSigner::Recovery(&kit(KIT_0).signing_key()),
        T0 + 2,
    )
    .unwrap();
    log.revoke_device(b.0, (&a.1, a.0), T0 + 3).unwrap();
    log.rotate_recovery(kit(KIT_1).verifying_key(), (&a.1, a.0), T0 + 4)
        .unwrap();
    let sealed = seal_root_for(&rotation_box(), 1, &c.1.public(), &SeededRng::new(BOX_SEED));
    log.rotate_root(1, vec![(c.0, sealed)], (&a.1, a.0), T0 + 5)
        .unwrap();
    // The joiner's name is encrypted under the root key of the current generation.
    let new_root = ark(ARK_1);
    log.add_device(
        d.0,
        d.1.public(),
        encrypt_name(&new_root, d.0, "Tablet", &names),
        LogSigner::Device(&c.1, c.0),
        T0 + 6,
    )
    .unwrap();
    log
}

/// The log as stored in the golden file: each entry as `u32 LE length || entry`.
pub fn encode_log(log: &DeviceLog) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in log.entries() {
        let bytes = entry.encode();
        out.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    out
}

/// Reverses [`encode_log`].
pub fn decode_log(mut bytes: &[u8]) -> Vec<LogEntry> {
    let mut entries = Vec::new();
    while let Some((len, rest)) = bytes.split_first_chunk::<4>() {
        let len = usize::try_from(u32::from_le_bytes(*len)).unwrap();
        let (entry, rest) = rest.split_at(len);
        entries.push(LogEntry::decode(entry).unwrap());
        bytes = rest;
    }
    assert!(bytes.is_empty(), "trailing bytes in the golden log");
    entries
}

pub fn account_id() -> AccountId {
    device_log().verify().unwrap().account_id
}

/// The blob the server stores for kit 1: the generation 1 root key wrapped under it.
pub fn recovery_blob() -> Vec<u8> {
    RecoveryBlob {
        // The account id is the hash of the Genesis alone.
        account_id: genesis(&SeededRng::new(NAME_SEED))
            .verify()
            .unwrap()
            .account_id,
        generation: 1,
        wrapped: kit(KIT_1).wrap_ark(&ark(ARK_1), &SeededRng::new(WRAP_SEED)),
    }
    .encode()
}

/// The joiner's identity.
pub fn identity() -> Identity {
    let d = device(4);
    Identity {
        device_id: d.0,
        keys: d.1.public(),
        name: "Tablet".to_owned(),
    }
}

/// What the sponsor (C) hands over.
pub fn bundle() -> Bundle {
    let log = device_log();
    Bundle {
        ark: ark(ARK_1),
        ark_generation: 1,
        account_id: log.verify().unwrap().account_id,
        server_url: SERVER.to_owned(),
        log: log.entries().iter().map(LogEntry::encode).collect(),
        recovery_id: kit(KIT_1).recovery_id(),
        recovery_blob: recovery_blob(),
        workspace_id: WorkspaceId::from_random_bytes([0x77; 16]),
        workspace_name: "Notes".to_owned(),
    }
}

/// Both sides after the agreement, with the clear offer, answer and reveal.
pub fn sessions() -> (Session, Session, Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(JOINER_SEED));
    joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) =
        Session::sponsor(MAILBOX, &offer, &SeededRng::new(SPONSOR_SEED)).unwrap();
    let reveal = joiner.accept_answer(&answer).unwrap();
    sponsor.accept_reveal(&reveal).unwrap();
    (joiner, sponsor, offer, answer, reveal)
}

pub fn generate() -> Vec<(&'static str, Vec<u8>)> {
    let (joiner, sponsor, offer, answer, reveal) = sessions();
    let identity = joiner.seal(
        MessageKind::Identity,
        SEQ_IDENTITY,
        &identity().encode(),
        &SeededRng::new(IDENTITY_SEED),
    );
    let bundle = sponsor.seal(
        MessageKind::Bundle,
        SEQ_BUNDLE,
        &bundle().encode(),
        &SeededRng::new(BUNDLE_SEED),
    );
    let bytes = [
        encode_log(&device_log()),
        recovery_blob(),
        offer,
        answer,
        reveal,
        identity,
        bundle,
    ];
    FILES.into_iter().zip(bytes).collect()
}
