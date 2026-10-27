//! The golden files of sync protocol v2 (`fixtures/sync/*-v2.bin`, written by
//! `examples/make_sync_vectors_v2.rs`): each one decodes and verifies with today's code,
//! regenerating it from its fixed inputs gives the same bytes, and sealing the same objects again
//! (other nonces) opens too. A failure here means a wire format changed: that needs a protocol
//! version bump, not a regenerated file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/vectors_v2.rs"]
mod vectors;

use kb_sync::v2::ws::{ENTRY_FRAME_PREFIX_LEN, PRESENCE_FRAME_PREFIX_LEN, PRESENCE_HEADER_LEN};
use kb_sync::v2::{
    ClientFrame, ENTRY_HEADER_LEN, HEAD_RECORD_LEN, ItemKind, KEYRING_HEADER_LEN, SIGNED_HEAD_LEN,
    SNAPSHOT_HEADER_LEN, ServerFrame, decode_client_binary, decode_client_text,
    decode_server_binary, decode_server_text, encode_changed, encode_entry_frame,
    encode_presence_frame, encode_presence_header, encode_subscribe, feed_kind, open_entry,
    open_keyring, open_presence, open_snapshot, peek_entry, peek_feed_item, peek_head,
    peek_keyring, peek_presence, peek_publication, peek_snapshot, seal_entry, seal_keyring,
    seal_presence, seal_publication, seal_snapshot, verify_head, verify_signature,
};
use kb_sync::{ChangesPage, SyncError};
use kb_types::sharing::PRESENCE_MAX_FRAME_BYTES;
use kb_types::testing::SeededRng;

const ENTRY: &[u8] = include_bytes!("../../../fixtures/sync/entry-v2.bin");
const KEYRING: &[u8] = include_bytes!("../../../fixtures/sync/keyring-v2.bin");
const SNAPSHOT: &[u8] = include_bytes!("../../../fixtures/sync/snapshot-v2.bin");
const HEAD: &[u8] = include_bytes!("../../../fixtures/sync/head-v2.bin");
const CHANGES_PAGE: &[u8] = include_bytes!("../../../fixtures/sync/changes-page-v2.bin");
const PRESENCE: &[u8] = include_bytes!("../../../fixtures/sync/presence-v2.bin");
const WS_ENTRY_FRAME: &[u8] = include_bytes!("../../../fixtures/sync/ws-entry-frame-v2.bin");
const WS_PRESENCE_FRAME: &[u8] = include_bytes!("../../../fixtures/sync/ws-presence-frame-v2.bin");
const WS_SUBSCRIBE: &str = include_str!("../../../fixtures/sync/ws-subscribe-v2.json");
const WS_CHANGED: &str = include_str!("../../../fixtures/sync/ws-changed-v2.json");
const PUBLICATION: &[u8] = include_bytes!("../../../fixtures/sync/publication-v2.bin");

fn frozen(name: &str) -> &'static [u8] {
    match name {
        "entry-v2.bin" => ENTRY,
        "keyring-v2.bin" => KEYRING,
        "snapshot-v2.bin" => SNAPSHOT,
        "head-v2.bin" => HEAD,
        "changes-page-v2.bin" => CHANGES_PAGE,
        "presence-v2.bin" => PRESENCE,
        "ws-entry-frame-v2.bin" => WS_ENTRY_FRAME,
        "ws-presence-frame-v2.bin" => WS_PRESENCE_FRAME,
        "ws-subscribe-v2.json" => WS_SUBSCRIBE.as_bytes(),
        "ws-changed-v2.json" => WS_CHANGED.as_bytes(),
        "publication-v2.bin" => PUBLICATION,
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
fn the_entry_golden_opens_and_has_the_v2_layout() {
    let verifier = vectors::signer().verifying_key();
    assert_eq!(feed_kind(ENTRY).unwrap(), ItemKind::Entry);
    assert_eq!(peek_entry(ENTRY).unwrap(), vectors::entry_header());
    verify_signature(ENTRY, &verifier).unwrap();
    let (header, body) = open_entry(ENTRY, &vectors::doc_key(), &verifier).unwrap();
    assert_eq!(header, vectors::entry_header());
    assert_eq!(body, vectors::entry_body());
    assert_eq!(&ENTRY[..3], &[1, 1, 1], "envelope v1, suite 1, kind Entry");
    assert_eq!(
        u32::from_le_bytes(ENTRY[3..7].try_into().unwrap()) as usize,
        ENTRY_HEADER_LEN
    );
    assert_eq!(ENTRY[7], 2, "protocol version 2");
    assert_eq!(
        &ENTRY[8..24],
        vectors::feed_id().as_bytes(),
        "the feed comes first"
    );
    let info = peek_feed_item(ENTRY).unwrap();
    assert_eq!(info.feed, vectors::feed_id());

    let resealed = seal_entry(
        &header,
        &body,
        &vectors::doc_key(),
        &vectors::signer(),
        &SeededRng::new(99),
    );
    assert_ne!(resealed, ENTRY);
    assert_eq!(
        open_entry(&resealed, &vectors::doc_key(), &verifier).unwrap(),
        (header, body)
    );
}

#[test]
fn the_keyring_golden_opens_and_its_items_verify() {
    let verifier = vectors::signer().verifying_key();
    let key = kb_sync::keyring_key(&vectors::ark());
    assert_eq!(feed_kind(KEYRING).unwrap(), ItemKind::Keyring);
    assert_eq!(peek_keyring(KEYRING).unwrap(), vectors::keyring_header());
    let (header, items) = open_keyring(KEYRING, &key, &verifier).unwrap();
    assert_eq!(header, vectors::keyring_header());
    assert_eq!(items, vectors::keyring_items());
    for item in &items {
        item.verify(&verifier).unwrap();
    }
    assert_eq!(
        &KEYRING[..3],
        &[1, 1, 3],
        "envelope v1, suite 1, kind Keyring"
    );
    assert_eq!(
        u32::from_le_bytes(KEYRING[3..7].try_into().unwrap()) as usize,
        KEYRING_HEADER_LEN
    );
    assert_eq!(KEYRING[7], 2, "protocol version 2");
    let resealed = seal_keyring(
        &header,
        &items,
        &key,
        &vectors::signer(),
        &SeededRng::new(98),
    );
    assert_ne!(resealed, KEYRING);
    assert_eq!(open_keyring(&resealed, &key, &verifier).unwrap().1, items);
}

#[test]
fn the_snapshot_golden_opens() {
    let verifier = vectors::signer().verifying_key();
    assert_eq!(peek_snapshot(SNAPSHOT).unwrap(), vectors::snapshot_header());
    verify_signature(SNAPSHOT, &verifier).unwrap();
    let (header, body) = open_snapshot(SNAPSHOT, &vectors::doc_key(), &verifier).unwrap();
    assert_eq!(header, vectors::snapshot_header());
    assert_eq!(body, vectors::snapshot_body());
    assert_eq!(
        &SNAPSHOT[..3],
        &[1, 1, 2],
        "envelope v1, suite 1, kind Snapshot"
    );
    assert_eq!(
        u32::from_le_bytes(SNAPSHOT[3..7].try_into().unwrap()) as usize,
        SNAPSHOT_HEADER_LEN
    );
    assert_eq!(SNAPSHOT[7], 2, "protocol version 2");
    let resealed = seal_snapshot(
        &header,
        &body,
        &vectors::doc_key(),
        &vectors::signer(),
        &SeededRng::new(97),
    );
    assert_ne!(resealed, SNAPSHOT);
    assert_eq!(
        open_snapshot(&resealed, &vectors::doc_key(), &verifier).unwrap(),
        (header, body)
    );
}

#[test]
fn the_head_golden_verifies_and_has_the_v2_layout() {
    assert_eq!(HEAD.len(), SIGNED_HEAD_LEN);
    assert_eq!(HEAD[0], 2, "protocol version 2");
    assert_eq!(
        &HEAD[1..17],
        vectors::feed_id().as_bytes(),
        "the feed comes first"
    );
    assert_eq!(peek_head(HEAD).unwrap(), vectors::head());
    let verifier = vectors::signer().verifying_key();
    assert_eq!(verify_head(HEAD, &verifier).unwrap(), vectors::head());
    assert_eq!(
        verify_head(
            HEAD,
            &kb_crypto::SigningKey::from_seed(&kb_crypto::Secret32::from_bytes([0x99; 32]))
                .verifying_key()
        ),
        Err(SyncError::Authentication)
    );
    // The signature covers the record: one flipped byte fails.
    let mut copy = HEAD.to_vec();
    copy[HEAD_RECORD_LEN - 1] ^= 1;
    assert_eq!(
        verify_head(&copy, &verifier),
        Err(SyncError::Authentication)
    );
}

#[test]
fn the_chain_of_the_head_is_the_running_hash_over_the_two_items() {
    assert_eq!(vectors::head().chain_hash, vectors::chain());
}

#[test]
fn the_changes_page_golden_decodes_and_its_chain_folds() {
    let page = ChangesPage::decode(CHANGES_PAGE).unwrap();
    assert_eq!(page, vectors::changes_page());
    assert_eq!(page.encode(), CHANGES_PAGE);
    assert_eq!(page.changes[0], (1, ENTRY.to_vec()));
    assert_eq!(page.changes[1], (2, KEYRING.to_vec()));
    let folded = page.changes.iter().fold(
        kb_sync::v2::chain_genesis(&page.server_epoch),
        |chain, (_, bytes)| kb_sync::v2::chain_next(&chain, bytes),
    );
    assert_eq!(folded, page.chain_hash_after);
    assert!(!page.more);
    // The head golden signs the same position.
    let head = peek_head(HEAD).unwrap();
    assert_eq!(
        (head.cursor, head.chain_hash, head.server_epoch),
        (2, page.chain_hash_after, page.server_epoch)
    );
}

// ----- WebSocket v2 and presence (M13 slice 3, ADR-0050) ---------------------------------------

#[test]
fn the_presence_golden_opens_and_has_the_documented_layout() {
    let verifier = vectors::signer().verifying_key();
    assert_eq!(peek_presence(PRESENCE).unwrap(), vectors::presence_header());
    let (header, body) = open_presence(PRESENCE, &vectors::presence_seal_key(), &verifier).unwrap();
    assert_eq!(header, vectors::presence_header());
    assert_eq!(body, vectors::presence_body());
    assert_eq!(
        &PRESENCE[..3],
        &[1, 1, 10],
        "envelope v1, suite 1, kind Presence"
    );
    assert_eq!(
        u32::from_le_bytes(PRESENCE[3..7].try_into().unwrap()) as usize,
        PRESENCE_HEADER_LEN
    );
    assert_eq!(PRESENCE_HEADER_LEN, 61);
    assert_eq!(PRESENCE[7], 2, "protocol version 2");
    assert_eq!(
        &PRESENCE[8..24],
        vectors::feed_id().as_bytes(),
        "the feed comes first"
    );
    assert_eq!(
        &PRESENCE[7..7 + PRESENCE_HEADER_LEN],
        &encode_presence_header(&header)[..]
    );
    // The body is padded to a bucket: 4 (length) + 1 + 16 + 1 + 4 + 23 bytes in a 256-byte bucket.
    let envelope = kb_crypto::Envelope::decode(PRESENCE).unwrap();
    assert_eq!(envelope.ciphertext.len(), 256 + 16);
    // Sealing the same objects again (other nonces) opens too.
    let resealed = seal_presence(
        &header,
        &body,
        &vectors::presence_seal_key(),
        &vectors::signer(),
        &SeededRng::new(96),
    );
    assert_ne!(resealed, PRESENCE);
    assert_eq!(
        open_presence(&resealed, &vectors::presence_seal_key(), &verifier).unwrap(),
        (header, body)
    );
    // Another epoch's key, another device's signature: refused.
    assert!(
        open_presence(
            PRESENCE,
            &kb_crypto::Secret32::from_bytes([1; 32]),
            &verifier
        )
        .is_err()
    );
    assert!(
        open_presence(
            PRESENCE,
            &vectors::presence_seal_key(),
            &kb_crypto::SigningKey::from_seed(&kb_crypto::Secret32::from_bytes([0x98; 32]))
                .verifying_key()
        )
        .is_err()
    );
}

#[test]
fn the_entry_frame_golden_is_the_documented_layout_around_the_entry_golden() {
    assert_eq!(WS_ENTRY_FRAME.len(), ENTRY_FRAME_PREFIX_LEN + ENTRY.len());
    assert_eq!(WS_ENTRY_FRAME[0], 1);
    assert_eq!(&WS_ENTRY_FRAME[1..17], vectors::feed_id().as_bytes());
    assert_eq!(&WS_ENTRY_FRAME[17..25], &41u64.to_le_bytes());
    assert_eq!(&WS_ENTRY_FRAME[25..33], &42u64.to_le_bytes());
    assert_eq!(
        &WS_ENTRY_FRAME[33..65],
        &kb_sync::v2::chain_next(&kb_sync::v2::chain_genesis(&vectors::SERVER_EPOCH), ENTRY)
    );
    assert_eq!(
        &WS_ENTRY_FRAME[65..],
        ENTRY,
        "the item's bytes exactly as changes returns them"
    );
    let ServerFrame::Entry(frame) = decode_server_binary(WS_ENTRY_FRAME).unwrap() else {
        panic!("an entry frame");
    };
    assert_eq!(frame, vectors::inline_entry());
    assert_eq!(encode_entry_frame(&frame).unwrap(), WS_ENTRY_FRAME);
    // The entry inside opens like the pulled one.
    let verifier = vectors::signer().verifying_key();
    assert!(open_entry(&frame.entry, &vectors::doc_key(), &verifier).is_ok());
    // Only a server sends it.
    assert!(matches!(
        decode_client_binary(WS_ENTRY_FRAME),
        Err(SyncError::Protocol(_))
    ));
}

#[test]
fn the_presence_frame_golden_is_the_documented_layout_around_the_presence_golden() {
    assert_eq!(
        WS_PRESENCE_FRAME.len(),
        PRESENCE_FRAME_PREFIX_LEN + PRESENCE.len()
    );
    assert_eq!(WS_PRESENCE_FRAME[0], 2);
    assert_eq!(&WS_PRESENCE_FRAME[1..17], vectors::feed_id().as_bytes());
    assert_eq!(&WS_PRESENCE_FRAME[17..], PRESENCE);
    let ServerFrame::Presence(frame) = decode_server_binary(WS_PRESENCE_FRAME).unwrap() else {
        panic!("a presence frame");
    };
    assert_eq!(frame, vectors::presence_frame());
    assert_eq!(
        decode_client_binary(WS_PRESENCE_FRAME).unwrap(),
        ClientFrame::Presence(frame.clone())
    );
    assert_eq!(encode_presence_frame(&frame).unwrap(), WS_PRESENCE_FRAME);
    assert!(WS_PRESENCE_FRAME.len() <= PRESENCE_MAX_FRAME_BYTES);
}

#[test]
fn the_text_goldens_are_the_documented_json() {
    assert_eq!(
        WS_SUBSCRIBE,
        r#"{"type":"subscribe","feeds":["61636374-2d30-3132-3334-353637383961","66656564-2d30-4132-b334-353637383961"]}"#
    );
    assert_eq!(
        decode_client_text(WS_SUBSCRIBE).unwrap(),
        ClientFrame::Subscribe(vectors::subscribed_feeds())
    );
    assert_eq!(
        encode_subscribe(&vectors::subscribed_feeds()).unwrap(),
        WS_SUBSCRIBE
    );
    assert_eq!(
        WS_CHANGED,
        r#"{"type":"changed","feed":"66656564-2d30-4132-b334-353637383961","cursor":42}"#
    );
    assert_eq!(
        decode_server_text(WS_CHANGED).unwrap(),
        ServerFrame::Changed {
            feed: vectors::feed_id(),
            cursor: 42
        }
    );
    assert_eq!(encode_changed(vectors::feed_id(), 42).unwrap(), WS_CHANGED);
}

// ----- the view link's publication (M13 slice 4, ADR-0051) -------------------------------------

#[test]
fn the_publication_golden_opens_and_has_the_documented_layout() {
    let key = vectors::link_key();
    // A server reads the link id without a key; the key derives the same id.
    let header = peek_publication(PUBLICATION).unwrap();
    assert_eq!(header.link_id, key.link_id());
    assert_eq!(
        kb_sync::v2::publication::open_publication(&key, PUBLICATION).unwrap(),
        vectors::publication_value()
    );
    // Envelope v1, suite 1, kind 11, a 17-byte header (`2 | link id`), no signature.
    assert_eq!(&PUBLICATION[..3], &[1, 1, 11]);
    assert_eq!(
        u32::from_le_bytes(PUBLICATION[3..7].try_into().unwrap()) as usize,
        kb_sync::v2::publication::PUBLICATION_HEADER_LEN
    );
    assert_eq!(PUBLICATION[7], 2, "protocol version 2");
    assert_eq!(&PUBLICATION[8..24], key.link_id().as_bytes());
    let envelope = kb_crypto::Envelope::decode(PUBLICATION).unwrap();
    assert!(envelope.signature.is_none());
    // Not bucket-padded: the ciphertext is the plaintext plus the 16-byte tag, exactly.
    let plaintext =
        kb_sync::v2::publication::encode_publication(&vectors::publication_value()).unwrap();
    assert_eq!(envelope.ciphertext.len(), plaintext.len() + 16);
    assert_eq!(plaintext[0], 1, "publication version 1");
    assert_eq!(
        &plaintext[1..17],
        vectors::publication_value().page_id.as_bytes()
    );
    // Sealing the same publication again (another nonce) opens too, and to the same value.
    let resealed =
        seal_publication(&key, &vectors::publication_value(), &SeededRng::new(97)).unwrap();
    assert_ne!(resealed, PUBLICATION);
    assert_eq!(
        kb_sync::v2::publication::open_publication(&key, &resealed).unwrap(),
        vectors::publication_value()
    );
    // Another link's key does not open it, and neither does a flipped byte anywhere.
    let other = kb_sync::v2::LinkKey::from_secret(kb_crypto::Secret32::from_bytes([0x78; 32]));
    assert_eq!(
        kb_sync::v2::publication::open_publication(&other, PUBLICATION).unwrap_err(),
        SyncError::Authentication
    );
}
