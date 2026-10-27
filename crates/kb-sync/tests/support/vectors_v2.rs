//! The fixed inputs behind the sync protocol v2 golden files in `fixtures/sync/`
//! (`entry-v2.bin`, `keyring-v2.bin`, `snapshot-v2.bin`, `head-v2.bin`, `changes-page-v2.bin`, and the
//! WebSocket v2 files of M13 slice 3: `presence-v2.bin`, `ws-entry-frame-v2.bin`,
//! `ws-presence-frame-v2.bin`, `ws-subscribe-v2.json`, `ws-changed-v2.json`, and the view link's
//! `publication-v2.bin` of M13 slice 4).
//!
//! Shared by `examples/make_sync_vectors_v2.rs` (writes the files) and `tests/goldens_v2.rs`
//! (checks them on every test run). Keys are fixed bytes, every random byte (nonces, padding,
//! wrapped-key nonces) comes from `SeededRng` with the seed named here, and Ed25519 signatures are
//! deterministic, so the output is reproducible byte for byte.

#![allow(dead_code)]

use kb_crypto::{Secret32, SigningKey};
use kb_keys::keyring::{KeySubject, new_space_key, tombstone};
use kb_keys::link::LinkKey;
use kb_keys::space_keys::{SpaceKey, presence_key};
use kb_keys::{Ark, DeviceKeys, KeyringItem};
use kb_sync::keyring_key;
use kb_sync::v2::{
    EntryHeader, Head, InlineEntry, KeyringHeader, PresenceBody, PresenceFrame, PresenceHeader,
    Publication, PublishedAttachment, PublishedPerson, SnapshotHeader, encode_changed,
    encode_entry_frame, encode_presence_frame, encode_subscribe, seal_entry, seal_keyring,
    seal_presence, seal_publication, seal_snapshot, sign_head,
};
use kb_sync::{ChangesPage, EntryBody, SnapshotBody};
use kb_types::testing::SeededRng;
use kb_types::{
    AccountId, AttachmentId, ChunkId, DeviceId, DocId, FeedId, PageId, PresenceState, SpaceId,
};

/// The document key of the entry and the snapshot.
pub const DOC_KEY: [u8; 32] = [0x11; 32];
/// The ARK whose `HKDF(ARK, "keyring")` seals the keyring entry.
pub const ARK: [u8; 32] = [0x22; 32];
/// Signing seed of the writing device.
pub const SIGNING_SEED: [u8; 32] = [0x33; 32];
/// Agreement seed of the writing device (only its public half matters here).
pub const AGREEMENT_SEED: [u8; 32] = [0x44; 32];
/// The server epoch of the head and the changes page.
pub const SERVER_EPOCH: [u8; 16] = [0x5e; 16];
/// The seed of the keyring items' wrapped-key nonces (the items are signed records, shared by every
/// object that carries them).
pub const ITEMS_SEED: u64 = 0x5_1002;
/// Fixed time of the keyring records and the head (Unix ms, 2026-10-04).
pub const T0: i64 = 1_791_072_000_000;

pub fn account_id() -> AccountId {
    AccountId::from_bytes(*b"acct-0123456789a")
}

pub fn doc_id() -> DocId {
    DocId::from_random_bytes(*b"doc-0123456789ab")
}

pub fn device_id() -> DeviceId {
    DeviceId::from_random_bytes(*b"dev-0123456789ab")
}

pub fn space_id() -> SpaceId {
    SpaceId::from_random_bytes(*b"spc-0123456789ab")
}

pub fn device() -> DeviceKeys {
    DeviceKeys::from_seeds(
        &Secret32::from_bytes(SIGNING_SEED),
        &Secret32::from_bytes(AGREEMENT_SEED),
    )
}

pub fn signer() -> SigningKey {
    device().signing().clone()
}

pub fn doc_key() -> Secret32 {
    Secret32::from_bytes(DOC_KEY)
}

pub fn ark() -> Ark {
    Ark::from_secret(Secret32::from_bytes(ARK))
}

pub fn entry_body() -> EntryBody {
    EntryBody {
        engine: 1,
        format_version: 1,
        min_reader_version: 1,
        doc_kind: 2,
        updates: vec![
            b"first update".to_vec(),
            Vec::new(),
            b"third update, longer than the first".to_vec(),
        ],
        version_vector: b"version vector".to_vec(),
    }
}

/// A space key record and a tombstone, signed by the device (deterministic: the record's wrapping
/// nonce comes from a seeded generator).
pub fn keyring_items() -> Vec<KeyringItem> {
    let rng = SeededRng::new(ITEMS_SEED ^ 0xff);
    let device = device();
    let (_, record) = new_space_key(&ark(), space_id(), 0, &device, device_id(), T0, &rng);
    let gone = tombstone(
        space_id(),
        KeySubject::Doc {
            doc_id: doc_id(),
            epoch: 0,
        },
        &device,
        device_id(),
        T0 + 1,
    );
    vec![KeyringItem::Record(record), KeyringItem::Tombstone(gone)]
}

pub fn snapshot_body() -> SnapshotBody {
    SnapshotBody {
        engine: 1,
        format_version: 1,
        min_reader_version: 1,
        doc_kind: 2,
        snapshot: b"a full document state".to_vec(),
        version_vector: b"version vector".to_vec(),
    }
}

/// Seeds of the generators each object is sealed with.
pub const ENTRY_SEED: u64 = 0x5_2001;
pub const KEYRING_SEED: u64 = 0x5_2002;
pub const SNAPSHOT_SEED: u64 = 0x5_2003;

/// The files, in the order the example writes them.
pub const FILES: [&str; 11] = [
    "entry-v2.bin",
    "keyring-v2.bin",
    "snapshot-v2.bin",
    "head-v2.bin",
    "changes-page-v2.bin",
    "presence-v2.bin",
    "ws-entry-frame-v2.bin",
    "ws-presence-frame-v2.bin",
    "ws-subscribe-v2.json",
    "ws-changed-v2.json",
    "publication-v2.bin",
];

/// The space key of epoch 2 whose presence key (`HKDF(SK, "presence", feed)`) seals the presence
/// envelope.
pub const SPACE_KEY: [u8; 32] = [0x66; 32];
/// The seed of the presence envelope's nonce and padding.
pub const PRESENCE_SEED: u64 = 0x5_2004;

/// The feed of every object.
pub fn feed_id() -> FeedId {
    FeedId::from_random_bytes(*b"feed-0123456789a")
}

pub fn entry_header() -> EntryHeader {
    EntryHeader {
        feed: feed_id(),
        account_id: account_id(),
        doc_id: doc_id(),
        device_id: device_id(),
        device_seq: 7,
        key_epoch: 2,
    }
}

pub fn keyring_header() -> KeyringHeader {
    KeyringHeader {
        feed: feed_id(),
        account_id: account_id(),
        device_id: device_id(),
        device_seq: 3,
        key_epoch: 0,
    }
}

pub fn snapshot_header() -> SnapshotHeader {
    SnapshotHeader {
        feed: feed_id(),
        account_id: account_id(),
        doc_id: doc_id(),
        device_id: device_id(),
        covers_cursor: 42,
        key_epoch: 2,
    }
}

pub fn entry() -> Vec<u8> {
    seal_entry(
        &entry_header(),
        &entry_body(),
        &doc_key(),
        &signer(),
        &SeededRng::new(ENTRY_SEED),
    )
}

pub fn keyring() -> Vec<u8> {
    seal_keyring(
        &keyring_header(),
        &keyring_items(),
        &keyring_key(&ark()),
        &signer(),
        &SeededRng::new(KEYRING_SEED),
    )
}

pub fn snapshot() -> Vec<u8> {
    seal_snapshot(
        &snapshot_header(),
        &snapshot_body(),
        &doc_key(),
        &signer(),
        &SeededRng::new(SNAPSHOT_SEED),
    )
}

/// The chain hash after the entry (cursor 1) and the keyring entry (cursor 2) under
/// [`SERVER_EPOCH`].
pub fn chain() -> [u8; 32] {
    let after_entry = kb_sync::v2::chain_next(&kb_sync::v2::chain_genesis(&SERVER_EPOCH), &entry());
    kb_sync::v2::chain_next(&after_entry, &keyring())
}

pub fn head() -> Head {
    Head {
        feed: feed_id(),
        account_id: account_id(),
        device_id: device_id(),
        cursor: 2,
        chain_hash: chain(),
        server_epoch: SERVER_EPOCH,
        at_ms: T0 + 2,
    }
}

/// The signed head.
pub fn signed_head() -> Vec<u8> {
    sign_head(&head(), &signer())
}

/// The first page of a feed holding the entry and the keyring entry (the framing of the page is
/// the crate root's, shared by every protocol version).
pub fn changes_page() -> ChangesPage {
    ChangesPage {
        server_epoch: SERVER_EPOCH,
        changes: vec![(1, entry()), (2, keyring())],
        chain_hash_after: chain(),
        more: false,
    }
}

// ----- WebSocket v2 and presence (M13 slice 3) -------------------------------------------------

/// The key the presence envelope is sealed under.
pub fn presence_seal_key() -> Secret32 {
    presence_key(
        &SpaceKey::from_secret(Secret32::from_bytes(SPACE_KEY)),
        feed_id(),
    )
}

pub fn presence_header() -> PresenceHeader {
    PresenceHeader {
        feed: feed_id(),
        account_id: account_id(),
        device_id: device_id(),
        epoch: 2,
        counter: 0x0000_018b_0000_0001,
    }
}

pub fn presence_body() -> PresenceBody {
    PresenceBody {
        doc_id: doc_id(),
        state: PresenceState::Editing,
        update: b"ephemeral cursor update".to_vec(),
    }
}

/// The presence envelope (kind 10), signed by the device.
pub fn presence() -> Vec<u8> {
    seal_presence(
        &presence_header(),
        &presence_body(),
        &presence_seal_key(),
        &signer(),
        &SeededRng::new(PRESENCE_SEED),
    )
}

/// The entry of the golden as an inline frame: the cursor before it is 41, its own 42, and the chain
/// is the running hash after it (the genesis of [`SERVER_EPOCH`] folded over the entry).
pub fn inline_entry() -> InlineEntry {
    InlineEntry {
        feed: feed_id(),
        prev_cursor: 41,
        cursor: 42,
        chain_after: kb_sync::v2::chain_next(&kb_sync::v2::chain_genesis(&SERVER_EPOCH), &entry()),
        entry: entry(),
    }
}

pub fn presence_frame() -> PresenceFrame {
    PresenceFrame {
        feed: feed_id(),
        envelope: presence(),
    }
}

/// The feeds of the subscribe frame: the personal feed (the account id) and the shared feed.
pub fn subscribed_feeds() -> Vec<FeedId> {
    vec![FeedId::from_account(account_id()), feed_id()]
}

// ----- the view link's publication (M13 slice 4, ADR-0051) ------------------------------------

/// The link key `K_link` of the publication (its fragment is what a URL carries).
pub const LINK_KEY: [u8; 32] = [0x77; 32];
/// The seed of the publication envelope's nonce.
pub const PUBLICATION_SEED: u64 = 0x5_2005;

pub fn link_key() -> LinkKey {
    LinkKey::from_secret(Secret32::from_bytes(LINK_KEY))
}

/// A publication with an icon, a cover, a snapshot blob, an image of two chunks, an empty file of
/// one chunk and two mentions (names with non-ASCII text).
pub fn publication_value() -> Publication {
    Publication {
        page_id: PageId::from_random_bytes(*b"page-0123456789a"),
        title: "Trip to Reykjav\u{ed}k".to_owned(),
        icon: Some("\u{1f30b}".to_owned()),
        cover: Some("attachment:61747461-6368-4d65-8e74-30313233343a".to_owned()),
        cover_y: 35,
        snapshot: b"KBD1 a snapshot blob without history".to_vec(),
        attachments: vec![
            PublishedAttachment {
                attachment_id: AttachmentId::from_random_bytes(*b"attach-012345678"),
                key: Secret32::from_bytes([0x5a; 32]),
                name: "geysir.png".to_owned(),
                mime: "image/png".to_owned(),
                size: 5_000_000,
                hash: [0xaa; 32],
                chunk_size: 4_194_304,
                chunks: vec![
                    ChunkId::from_bytes([0xc1; 32]),
                    ChunkId::from_bytes([0xc2; 32]),
                ],
            },
            PublishedAttachment {
                attachment_id: AttachmentId::from_random_bytes(*b"attach-987654321"),
                key: Secret32::from_bytes([0x5b; 32]),
                name: "empty.txt".to_owned(),
                mime: "text/plain".to_owned(),
                size: 0,
                hash: [0xbb; 32],
                chunk_size: 4_194_304,
                chunks: vec![ChunkId::from_bytes([0xc3; 32])],
            },
        ],
        mentions: vec![
            PublishedPerson {
                account_id: AccountId::from_bytes(*b"acct-aaaaaaaaaaa"),
                name: "Sigr\u{ed}\u{f0}ur".to_owned(),
            },
            PublishedPerson {
                account_id: account_id(),
                name: "Ben".to_owned(),
            },
        ],
    }
}

/// The sealed publication (kind 11, unsigned).
pub fn publication() -> Vec<u8> {
    seal_publication(
        &link_key(),
        &publication_value(),
        &SeededRng::new(PUBLICATION_SEED),
    )
    .unwrap()
}

/// Every file with its name, in [`FILES`] order.
pub fn generate() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (FILES[0], entry()),
        (FILES[1], keyring()),
        (FILES[2], snapshot()),
        (FILES[3], signed_head()),
        (FILES[4], changes_page().encode()),
        (FILES[5], presence()),
        (FILES[6], encode_entry_frame(&inline_entry()).unwrap()),
        (FILES[7], encode_presence_frame(&presence_frame()).unwrap()),
        (
            FILES[8],
            encode_subscribe(&subscribed_feeds()).unwrap().into_bytes(),
        ),
        (
            FILES[9],
            encode_changed(feed_id(), 42).unwrap().into_bytes(),
        ),
        (FILES[10], publication()),
    ]
}
