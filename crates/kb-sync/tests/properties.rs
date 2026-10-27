//! Property tests: every wire object round-trips, and no decoder panics or accepts
//! arbitrary bytes it should not (G8: every decoder is typed and total).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::{Secret32, SigningKey};
use kb_sync::v2::{
    EntryHeader, Head, SnapshotHeader, feed_kind, open_entry, open_keyring, open_snapshot,
    peek_entry, peek_feed_item, peek_head, peek_keyring, peek_snapshot, seal_entry, seal_snapshot,
    sign_head, verify_head, verify_signature,
};
use kb_sync::{ChangesPage, EntryBody, SnapshotBody, SnapshotsPage};
use kb_types::testing::SeededRng;
use kb_types::{AccountId, DeviceId, DocId, FeedId};
use proptest::prelude::*;

fn key() -> Secret32 {
    Secret32::from_bytes([0x11; 32])
}

fn signer() -> SigningKey {
    SigningKey::from_seed(&Secret32::from_bytes([0x22; 32]))
}

fn blob() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..64)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn entries_round_trip(
        ids in any::<([u8; 16], [u8; 16], [u8; 16], [u8; 16])>(),
        device_seq in any::<u64>(),
        key_epoch in any::<u32>(),
        prefix in any::<(u8, u16, u16, u8)>(),
        updates in proptest::collection::vec(blob(), 0..5),
        version_vector in blob(),
        seed in any::<u64>(),
    ) {
        let header = EntryHeader {
            feed: FeedId::from_bytes(ids.3),
            account_id: AccountId::from_bytes(ids.0),
            doc_id: DocId::from_bytes(ids.1),
            device_id: DeviceId::from_bytes(ids.2),
            device_seq,
            key_epoch,
        };
        let body = EntryBody {
            engine: prefix.0,
            format_version: prefix.1,
            min_reader_version: prefix.2,
            doc_kind: prefix.3,
            updates,
            version_vector,
        };
        let bytes = seal_entry(&header, &body, &key(), &signer(), &SeededRng::new(seed));
        prop_assert_eq!(peek_entry(&bytes).unwrap(), header.clone());
        prop_assert_eq!(
            open_entry(&bytes, &key(), &signer().verifying_key()).unwrap(),
            (header, body)
        );
    }

    #[test]
    fn snapshots_round_trip(
        covers in any::<u64>(),
        snapshot in proptest::collection::vec(any::<u8>(), 0..600),
        version_vector in blob(),
        seed in any::<u64>(),
    ) {
        let header = SnapshotHeader {
            feed: FeedId::from_bytes([4; 16]),
            account_id: AccountId::from_bytes([1; 16]),
            doc_id: DocId::from_bytes([2; 16]),
            device_id: DeviceId::from_bytes([3; 16]),
            covers_cursor: covers,
            key_epoch: 0,
        };
        let body = SnapshotBody {
            engine: 1,
            format_version: 1,
            min_reader_version: 1,
            doc_kind: 1,
            snapshot,
            version_vector,
        };
        let bytes = seal_snapshot(&header, &body, &key(), &signer(), &SeededRng::new(seed));
        prop_assert_eq!(
            open_snapshot(&bytes, &key(), &signer().verifying_key()).unwrap(),
            (header, body)
        );
    }

    #[test]
    fn heads_round_trip(
        ids in any::<([u8; 16], [u8; 16], [u8; 16])>(),
        cursor in any::<u64>(),
        chain_hash in any::<[u8; 32]>(),
        server_epoch in any::<[u8; 16]>(),
        at_ms in any::<i64>(),
    ) {
        let head = Head {
            feed: FeedId::from_bytes(ids.2),
            account_id: AccountId::from_bytes(ids.0),
            device_id: DeviceId::from_bytes(ids.1),
            cursor,
            chain_hash,
            server_epoch,
            at_ms,
        };
        let bytes = sign_head(&head, &signer());
        prop_assert_eq!(peek_head(&bytes).unwrap(), head.clone());
        prop_assert_eq!(verify_head(&bytes, &signer().verifying_key()).unwrap(), head);
    }

    #[test]
    fn pages_round_trip(
        epoch in any::<[u8; 16]>(),
        items in proptest::collection::vec((1u64..1000, blob()), 0..20),
        chain in any::<[u8; 32]>(),
        more in any::<bool>(),
    ) {
        // Strictly increasing cursors, as the decoder requires.
        let mut cursor = 0u64;
        let changes: Vec<(u64, Vec<u8>)> = items
            .into_iter()
            .map(|(step, bytes)| {
                cursor += step;
                (cursor, bytes)
            })
            .collect();
        let page = ChangesPage { server_epoch: epoch, changes, chain_hash_after: chain, more };
        prop_assert_eq!(ChangesPage::decode(&page.encode()).unwrap(), page.clone());
        let snapshots = SnapshotsPage {
            server_epoch: epoch,
            snapshots: page.changes.into_iter().map(|(_, b)| b).collect(),
        };
        prop_assert_eq!(SnapshotsPage::decode(&snapshots.encode()).unwrap(), snapshots);
    }

    #[test]
    fn decoders_never_panic_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        let verifier = signer().verifying_key();
        let _ = feed_kind(&bytes);
        let _ = peek_feed_item(&bytes);
        let _ = peek_entry(&bytes);
        let _ = peek_keyring(&bytes);
        let _ = peek_snapshot(&bytes);
        let _ = peek_head(&bytes);
        let _ = ChangesPage::decode(&bytes);
        let _ = SnapshotsPage::decode(&bytes);
        prop_assert!(open_entry(&bytes, &key(), &verifier).is_err());
        prop_assert!(open_keyring(&bytes, &key(), &verifier).is_err());
        prop_assert!(open_snapshot(&bytes, &key(), &verifier).is_err());
        prop_assert!(verify_head(&bytes, &verifier).is_err());
        prop_assert!(verify_signature(&bytes, &verifier).is_err());
    }

    #[test]
    fn decoders_never_panic_on_mutated_valid_objects(
        seed in any::<u64>(),
        position in any::<prop::sample::Index>(),
        value in any::<u8>(),
        cut in any::<prop::sample::Index>(),
    ) {
        let header = EntryHeader {
            feed: FeedId::from_bytes([4; 16]),
            account_id: AccountId::from_bytes([1; 16]),
            doc_id: DocId::from_bytes([2; 16]),
            device_id: DeviceId::from_bytes([3; 16]),
            device_seq: 1,
            key_epoch: 0,
        };
        let body = EntryBody {
            engine: 1,
            format_version: 1,
            min_reader_version: 1,
            doc_kind: 1,
            updates: vec![b"update".to_vec()],
            version_vector: Vec::new(),
        };
        let bytes = seal_entry(&header, &body, &key(), &signer(), &SeededRng::new(seed));
        let mut mutated = bytes.clone();
        let i = position.index(mutated.len());
        mutated[i] = value;
        let mutated = &mutated[..=cut.index(mutated.len()).max(i)];
        let opened = open_entry(mutated, &key(), &signer().verifying_key());
        if mutated == bytes.as_slice() {
            prop_assert!(opened.is_ok());
        } else {
            prop_assert!(opened.is_err());
        }
        let page = ChangesPage {
            server_epoch: [0; 16],
            changes: vec![(1, bytes)],
            chain_hash_after: [0; 32],
            more: false,
        }
        .encode();
        let mut page = page;
        let j = position.index(page.len());
        page[j] = value;
        let _ = ChangesPage::decode(&page[..=cut.index(page.len()).max(j)]);
    }
}

// ----- WebSocket v2 and presence (M13 slice 3) -------------------------------------------------

use kb_sync::v2::{
    ClientFrame, InlineEntry, PresenceBody, PresenceFrame, PresenceHeader, ServerFrame,
    decode_client_binary, decode_client_text, decode_server_binary, decode_server_text,
    encode_changed, encode_entry_frame, encode_presence_frame, encode_subscribe, open_presence,
    peek_presence, seal_presence,
};
use kb_types::PresenceState;
use kb_types::sharing::{INLINE_ENTRY_MAX_BYTES, MAX_FEEDS_PER_ACCOUNT, PRESENCE_MAX_FRAME_BYTES};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn entry_frames_round_trip(
        feed in any::<[u8; 16]>(),
        prev_cursor in any::<u64>(),
        cursor in any::<u64>(),
        chain_after in any::<[u8; 32]>(),
        entry in proptest::collection::vec(any::<u8>(), 0..300),
    ) {
        let frame = InlineEntry {
            feed: FeedId::from_bytes(feed),
            prev_cursor,
            cursor,
            chain_after,
            entry,
        };
        let bytes = encode_entry_frame(&frame).unwrap();
        prop_assert_eq!(decode_server_binary(&bytes).unwrap(), ServerFrame::Entry(frame));
        // Only a server sends entry frames.
        prop_assert!(decode_client_binary(&bytes).is_err());
    }

    #[test]
    fn presence_frames_round_trip(
        feed in any::<[u8; 16]>(),
        envelope in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        let frame = PresenceFrame { feed: FeedId::from_bytes(feed), envelope };
        let bytes = encode_presence_frame(&frame).unwrap();
        prop_assert_eq!(
            decode_server_binary(&bytes).unwrap(),
            ServerFrame::Presence(frame.clone())
        );
        prop_assert_eq!(decode_client_binary(&bytes).unwrap(), ClientFrame::Presence(frame));
    }

    #[test]
    fn text_frames_round_trip(
        feeds in proptest::collection::vec(any::<[u8; 16]>(), 0..=MAX_FEEDS_PER_ACCOUNT),
        feed in any::<[u8; 16]>(),
        cursor in any::<u64>(),
    ) {
        let feeds: Vec<FeedId> = feeds.into_iter().map(FeedId::from_bytes).collect();
        let text = encode_subscribe(&feeds).unwrap();
        prop_assert_eq!(decode_client_text(&text).unwrap(), ClientFrame::Subscribe(feeds));
        let changed = encode_changed(FeedId::from_bytes(feed), cursor).unwrap();
        prop_assert_eq!(
            decode_server_text(&changed).unwrap(),
            ServerFrame::Changed { feed: FeedId::from_bytes(feed), cursor }
        );
    }

    #[test]
    fn websocket_decoders_never_panic_on_any_input(
        bytes in proptest::collection::vec(any::<u8>(), 0..5000),
        text in ".{0,300}",
    ) {
        let server = decode_server_binary(&bytes);
        let client = decode_client_binary(&bytes);
        // The two directions agree on presence frames and refuse what is over the limits.
        if bytes.len() > INLINE_ENTRY_MAX_BYTES + 65 || (bytes.first() == Some(&2) && bytes.len() > PRESENCE_MAX_FRAME_BYTES) {
            prop_assert!(server.is_err() && client.is_err());
        }
        if bytes.first() == Some(&1) {
            prop_assert!(client.is_err());
        }
        let _ = decode_server_text(&text);
        let _ = decode_client_text(&text);
        let _ = peek_presence(&bytes);
        let _ = open_presence(&bytes, &key(), &signer().verifying_key());
    }

    #[test]
    fn presence_envelopes_round_trip_and_do_not_open_when_changed(
        ids in any::<([u8; 16], [u8; 16], [u8; 16], [u8; 16])>(),
        epoch in any::<u32>(),
        counter in any::<u64>(),
        state in 1u8..=3,
        update in proptest::collection::vec(any::<u8>(), 0..600),
        seed in any::<u64>(),
        position in any::<prop::sample::Index>(),
    ) {
        let header = PresenceHeader {
            feed: FeedId::from_bytes(ids.0),
            account_id: AccountId::from_bytes(ids.1),
            device_id: DeviceId::from_bytes(ids.2),
            epoch,
            counter,
        };
        let body = PresenceBody {
            doc_id: DocId::from_bytes(ids.3),
            state: PresenceState::from_byte(state).unwrap(),
            update,
        };
        let sealed = seal_presence(&header, &body, &key(), &signer(), &SeededRng::new(seed));
        prop_assert_eq!(peek_presence(&sealed).unwrap(), header.clone());
        let verifier = signer().verifying_key();
        prop_assert_eq!(
            open_presence(&sealed, &key(), &verifier).unwrap(),
            (header, body)
        );
        // Anything past the envelope's three-byte prefix that changes breaks the signature.
        let mut changed = sealed.clone();
        let i = 3 + position.index(changed.len() - 3);
        changed[i] ^= 0x01;
        prop_assert!(open_presence(&changed, &key(), &verifier).is_err());
    }
}
