//! The objects the protocol versions share: the pages of changes and snapshots, the authentication
//! message, the chain hash. (The headers and the sealed objects are tested in `v2/tests.rs`.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::{Secret32, SigningKey};
use kb_types::sync::{CHANGES_PAGE_LIMIT, MAX_WIRE_BYTES};

use super::*;

fn key(byte: u8) -> Secret32 {
    Secret32::from_bytes([byte; 32])
}

fn signer(byte: u8) -> SigningKey {
    SigningKey::from_seed(&key(byte))
}

fn is_encoding<T: core::fmt::Debug>(result: Result<T, SyncError>) -> bool {
    matches!(result, Err(SyncError::Encoding(_)))
}

#[test]
fn chain_hash_is_blake3_of_the_previous_value_and_the_bytes() {
    let epoch = [0x01; 16];
    assert_eq!(chain_genesis(&epoch), kb_crypto::hash(&epoch));
    let genesis = chain_genesis(&epoch);
    let mut joined = genesis.to_vec();
    joined.extend_from_slice(b"entry bytes");
    assert_eq!(
        chain_next(&genesis, b"entry bytes"),
        kb_crypto::hash(&joined)
    );
    assert_ne!(chain_next(&genesis, b"a"), chain_next(&genesis, b"b"));
    assert_ne!(chain_genesis(&[0x02; 16]), genesis);
}

// ----- pages --------------------------------------------------------------------------------

fn page() -> ChangesPage {
    ChangesPage {
        server_epoch: [0x5e; 16],
        changes: vec![
            (1, b"one".to_vec()),
            (3, Vec::new()),
            (u64::MAX, b"last".to_vec()),
        ],
        chain_hash_after: [0xc4; 32],
        more: true,
    }
}

#[test]
fn changes_page_is_the_documented_layout() {
    let bytes = page().encode();
    let mut expected = vec![1u8];
    expected.extend_from_slice(&[0x5e; 16]);
    expected.extend_from_slice(&3u32.to_le_bytes());
    expected.extend_from_slice(&1u64.to_le_bytes());
    expected.extend_from_slice(&3u32.to_le_bytes());
    expected.extend_from_slice(b"one");
    expected.extend_from_slice(&3u64.to_le_bytes());
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(&u64::MAX.to_le_bytes());
    expected.extend_from_slice(&4u32.to_le_bytes());
    expected.extend_from_slice(b"last");
    expected.extend_from_slice(&[0xc4; 32]);
    expected.push(1);
    assert_eq!(bytes, expected);
    assert_eq!(ChangesPage::decode(&bytes).unwrap(), page());

    let empty = ChangesPage {
        server_epoch: [0; 16],
        changes: Vec::new(),
        chain_hash_after: [9; 32],
        more: false,
    };
    assert_eq!(empty.encode().len(), 1 + 16 + 4 + 32 + 1);
    assert_eq!(ChangesPage::decode(&empty.encode()).unwrap(), empty);
}

#[test]
fn changes_page_decoder_refuses_malformed_pages() {
    let bytes = page().encode();
    for len in 0..bytes.len() {
        assert!(
            is_encoding(ChangesPage::decode(&bytes[..len])),
            "prefix of {len}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(is_encoding(ChangesPage::decode(&longer)));
    let mut version = bytes.clone();
    version[0] = 2;
    assert!(is_encoding(ChangesPage::decode(&version)));
    let mut more = bytes.clone();
    *more.last_mut().unwrap() = 2;
    assert!(is_encoding(ChangesPage::decode(&more)));

    let too_many = ChangesPage {
        changes: (1..=u64::from(CHANGES_PAGE_LIMIT) + 1)
            .map(|c| (c, Vec::new()))
            .collect(),
        ..page()
    };
    assert!(matches!(
        ChangesPage::decode(&too_many.encode()),
        Err(SyncError::Protocol(_))
    ));
    let at_limit = ChangesPage {
        changes: (1..=u64::from(CHANGES_PAGE_LIMIT))
            .map(|c| (c, Vec::new()))
            .collect(),
        ..page()
    };
    assert_eq!(ChangesPage::decode(&at_limit.encode()).unwrap(), at_limit);
    // A count beyond the limit is a protocol violation even when the bytes are missing.
    let mut claims = bytes.clone();
    claims[17..21].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        ChangesPage::decode(&claims),
        Err(SyncError::Protocol(_))
    ));

    for cursors in [[2u64, 2], [3, 2]] {
        let unordered = ChangesPage {
            changes: cursors.iter().map(|c| (*c, Vec::new())).collect(),
            ..page()
        };
        assert!(matches!(
            ChangesPage::decode(&unordered.encode()),
            Err(SyncError::Protocol(_))
        ));
    }
    let oversized = ChangesPage {
        changes: vec![(1, vec![0; usize::try_from(MAX_WIRE_BYTES).unwrap() + 1])],
        ..page()
    };
    assert!(matches!(
        ChangesPage::decode(&oversized.encode()),
        Err(SyncError::Protocol(_))
    ));
}

#[test]
fn snapshots_page_is_the_documented_layout() {
    let snapshots = SnapshotsPage {
        server_epoch: [0x5e; 16],
        snapshots: vec![b"ab".to_vec(), Vec::new()],
    };
    let bytes = snapshots.encode();
    let mut expected = vec![1u8];
    expected.extend_from_slice(&[0x5e; 16]);
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(b"ab");
    expected.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(bytes, expected);
    assert_eq!(SnapshotsPage::decode(&bytes).unwrap(), snapshots);
    for len in 0..bytes.len() {
        assert!(
            is_encoding(SnapshotsPage::decode(&bytes[..len])),
            "prefix of {len}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(is_encoding(SnapshotsPage::decode(&longer)));
    let mut version = bytes.clone();
    version[0] = 0;
    assert!(is_encoding(SnapshotsPage::decode(&version)));
    // No count limit: every document's snapshot fits in one page.
    let many = SnapshotsPage {
        server_epoch: [0; 16],
        snapshots: vec![Vec::new(); 1000],
    };
    assert_eq!(SnapshotsPage::decode(&many.encode()).unwrap(), many);
    let oversized = SnapshotsPage {
        server_epoch: [0; 16],
        snapshots: vec![vec![0; usize::try_from(MAX_WIRE_BYTES).unwrap() + 1]],
    };
    assert!(matches!(
        SnapshotsPage::decode(&oversized.encode()),
        Err(SyncError::Protocol(_))
    ));
}

// ----- authentication -------------------------------------------------------------------------

#[test]
fn auth_message_is_nonce_then_epoch_signed_with_context_auth() {
    let nonce = [0x11; 32];
    let epoch = [0x22; 16];
    let message = auth_message(&nonce, &epoch);
    assert_eq!(message.len(), 48);
    assert_eq!(&message[..32], &nonce);
    assert_eq!(&message[32..], &epoch);
    let signature = sign_auth(&message, &signer(6));
    let verifier = signer(6).verifying_key();
    verify_auth(&message, &signature, &verifier).unwrap();
    kb_crypto::verify(&verifier, "auth", &message, &signature).unwrap();
    assert!(kb_crypto::verify(&verifier, "head", &message, &signature).is_err());
    assert_eq!(
        verify_auth(&auth_message(&nonce, &[0x23; 16]), &signature, &verifier),
        Err(SyncError::Authentication)
    );
    assert_eq!(
        verify_auth(&message, &signature, &signer(7).verifying_key()),
        Err(SyncError::Authentication)
    );
}
