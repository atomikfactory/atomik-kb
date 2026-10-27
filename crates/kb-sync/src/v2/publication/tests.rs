//! The publication module: the header, the upload framing, the helpers, the plaintext codec and
//! seal and open (the golden is `tests/goldens_v2.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::{SigningKey, seal_envelope};
use kb_types::testing::SeededRng;

use super::*;

fn link(byte: u8) -> LinkId {
    LinkId::from_bytes([byte; 16])
}

fn chunk(byte: u8) -> ChunkId {
    ChunkId::from_bytes([byte; 32])
}

/// A publication envelope made by hand (what `seal_publication` will produce): kind 11, the header of
/// the link, an arbitrary body, unsigned.
fn envelope_for(link_id: LinkId) -> Vec<u8> {
    let key = LinkKey::from_secret(Secret32::from_bytes([7; 32]));
    seal_envelope(
        EnvelopeKind::Publication,
        &encode_publication_header(&PublicationHeader { link_id }),
        &key.publication_key(),
        b"body",
        None,
        &SeededRng::new(1),
    )
}

#[test]
fn the_header_is_the_protocol_version_and_the_link_id() {
    let header = PublicationHeader {
        link_id: link(0xcd),
    };
    let bytes = encode_publication_header(&header);
    assert_eq!(bytes.len(), PUBLICATION_HEADER_LEN);
    assert_eq!(bytes[0], 2);
    assert_eq!(&bytes[1..], &[0xcd; 16]);
    assert_eq!(decode_publication_header(&bytes).unwrap(), header);
}

#[test]
fn a_server_reads_the_link_id_of_a_publication_without_a_key() {
    let bytes = envelope_for(link(3));
    assert_eq!(
        peek_publication(&bytes).unwrap(),
        PublicationHeader { link_id: link(3) }
    );
    // An envelope of another kind is not a publication.
    let other = seal_envelope(
        EnvelopeKind::Entry,
        &encode_publication_header(&PublicationHeader { link_id: link(3) }),
        &Secret32::from_bytes([7; 32]),
        b"body",
        None,
        &SeededRng::new(2),
    );
    assert!(matches!(
        peek_publication(&other),
        Err(SyncError::Encoding(_))
    ));
    // A signed publication is refused: a publication has no signer.
    let signed = seal_envelope(
        EnvelopeKind::Publication,
        &encode_publication_header(&PublicationHeader { link_id: link(3) }),
        &Secret32::from_bytes([7; 32]),
        b"body",
        Some(&SigningKey::from_seed(&Secret32::from_bytes([8; 32]))),
        &SeededRng::new(3),
    );
    assert!(matches!(
        peek_publication(&signed),
        Err(SyncError::Encoding(_))
    ));
    // A header of the old protocol version, a short header and garbage are errors, never a panic.
    let mut v1 = encode_publication_header(&PublicationHeader { link_id: link(3) });
    v1[0] = 1;
    let old = seal_envelope(
        EnvelopeKind::Publication,
        &v1,
        &Secret32::from_bytes([7; 32]),
        b"body",
        None,
        &SeededRng::new(4),
    );
    assert!(matches!(
        peek_publication(&old),
        Err(SyncError::Protocol(_))
    ));
    let short = seal_envelope(
        EnvelopeKind::Publication,
        &[2, 1, 2, 3],
        &Secret32::from_bytes([7; 32]),
        b"body",
        None,
        &SeededRng::new(5),
    );
    assert!(matches!(
        peek_publication(&short),
        Err(SyncError::Encoding(_))
    ));
    for len in 0..bytes.len() {
        assert!(peek_publication(&bytes[..len]).is_err(), "{len}");
    }
    assert!(peek_publication(&[0xff; 64]).is_err());
}

#[test]
fn the_upload_has_the_documented_layout() {
    let put = LinkPut {
        feed: FeedId::from_bytes([0xf1; 16]),
        expires_at: Some(0x0102_0304_0506_0708),
        chunks: vec![chunk(0xc1), chunk(0xc2)],
        publication: vec![9, 8, 7],
    };
    let bytes = encode_link_put(&put).unwrap();
    let mut expected = vec![2u8];
    expected.extend_from_slice(&[0xf1; 16]);
    expected.push(1);
    expected.extend_from_slice(&0x0102_0304_0506_0708_i64.to_le_bytes());
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(&[0xc1; 32]);
    expected.extend_from_slice(&[0xc2; 32]);
    expected.extend_from_slice(&[9, 8, 7]);
    assert_eq!(bytes, expected);
    assert_eq!(bytes.len(), LINK_PUT_PREFIX_LEN + 2 * 32 + 3);
    assert_eq!(decode_link_put(&bytes).unwrap(), put);

    // No expiry and no chunks: the flag is clear and the expiry field is zero.
    let plain = LinkPut {
        feed: FeedId::from_bytes([0xf2; 16]),
        expires_at: None,
        chunks: Vec::new(),
        publication: vec![1],
    };
    let bytes = encode_link_put(&plain).unwrap();
    assert_eq!(bytes[17], 0, "flags");
    assert_eq!(&bytes[18..26], &[0u8; 8]);
    assert_eq!(bytes.len(), LINK_PUT_PREFIX_LEN + 1);
    assert_eq!(decode_link_put(&bytes).unwrap(), plain);

    // An expiry of zero (the epoch) is a real expiry when the flag says so.
    let epoch = LinkPut {
        expires_at: Some(0),
        ..plain
    };
    assert_eq!(
        decode_link_put(&encode_link_put(&epoch).unwrap()).unwrap(),
        epoch
    );
}

#[test]
fn an_upload_that_breaks_a_rule_is_refused() {
    let put = LinkPut {
        feed: FeedId::from_bytes([1; 16]),
        expires_at: Some(5),
        chunks: vec![chunk(1)],
        publication: vec![1, 2, 3],
    };
    let good = encode_link_put(&put).unwrap();

    // Every truncation inside the prefix or the hashes, and the missing publication.
    for len in 0..LINK_PUT_PREFIX_LEN + 32 + 1 {
        assert!(decode_link_put(&good[..len]).is_err(), "{len}");
    }
    // Unknown flag bits, an expiry without its flag, a v1 object.
    let mut bad = good.clone();
    bad[17] = 3;
    assert!(matches!(decode_link_put(&bad), Err(SyncError::Encoding(_))));
    let mut bad = good.clone();
    bad[17] = 0;
    assert!(matches!(decode_link_put(&bad), Err(SyncError::Encoding(_))));
    let mut bad = good.clone();
    bad[0] = 1;
    assert!(matches!(decode_link_put(&bad), Err(SyncError::Protocol(_))));
    // A count the input cannot hold, and one over the limit, whatever follows.
    let mut bad = good.clone();
    bad[26..30].copy_from_slice(&9u32.to_le_bytes());
    assert!(matches!(decode_link_put(&bad), Err(SyncError::Encoding(_))));
    let mut bad = good;
    let over = u32::try_from(LINK_MAX_CHUNKS + 1).unwrap();
    bad[26..30].copy_from_slice(&over.to_le_bytes());
    assert!(matches!(decode_link_put(&bad), Err(SyncError::Protocol(_))));
    let mut bad = vec![0xffu8; 4096];
    bad[0] = 2;
    bad[17] = 0;
    bad[18..26].copy_from_slice(&[0; 8]);
    bad[26..30].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_link_put(&bad).is_err());

    // The encoder refuses what the decoder would.
    let too_many = LinkPut {
        chunks: vec![chunk(2); LINK_MAX_CHUNKS + 1],
        ..put.clone()
    };
    assert!(matches!(
        encode_link_put(&too_many),
        Err(SyncError::Protocol(_))
    ));
    let at_limit = LinkPut {
        chunks: vec![chunk(2); LINK_MAX_CHUNKS],
        ..put.clone()
    };
    assert_eq!(
        decode_link_put(&encode_link_put(&at_limit).unwrap()).unwrap(),
        at_limit
    );
    let empty = LinkPut {
        publication: Vec::new(),
        ..put
    };
    assert!(matches!(
        encode_link_put(&empty),
        Err(SyncError::Protocol(_))
    ));
}

#[test]
fn a_publication_lists_the_chunks_of_its_attachments_in_order() {
    let attachment = |id: u8, chunks: Vec<ChunkId>| PublishedAttachment {
        attachment_id: AttachmentId::from_bytes([id; 16]),
        key: Secret32::from_bytes([0x5a; 32]),
        name: format!("file{id}"),
        mime: "image/png".into(),
        size: 10,
        hash: [id; 32],
        chunk_size: 4 * 1024 * 1024,
        chunks,
    };
    let publication = Publication {
        page_id: PageId::from_bytes([1; 16]),
        title: "T".into(),
        icon: None,
        cover: None,
        cover_y: 50,
        snapshot: Vec::new(),
        attachments: vec![
            attachment(1, vec![chunk(1), chunk(2)]),
            attachment(2, vec![chunk(3)]),
        ],
        mentions: Vec::new(),
    };
    assert_eq!(publication.chunk_ids(), vec![chunk(1), chunk(2), chunk(3)]);
    // The attachment key is never printed, in hex or as numbers.
    let shown = format!("{publication:?}");
    assert!(
        !shown.contains("5a5a") && !shown.contains("90, 90"),
        "{shown}"
    );
}

#[test]
fn the_limits_fit_one_upload() {
    const { assert!(LINK_MAX_CHUNKS * 32 + LINK_PUT_PREFIX_LEN < 64 * 1024) };
    const { assert!(PUBLICATION_HEADER_LEN == 17) };
    assert_eq!(PUBLICATION_VERSION, 1);
    assert_eq!(PUBLICATION_COVER_Y_MAX, 100);
}

// ----- the plaintext codec, seal and open (A's part of slice 4) ---------------------------------

fn attachment(id: u8, size: u64, chunk_size: u32, chunks: Vec<ChunkId>) -> PublishedAttachment {
    PublishedAttachment {
        attachment_id: AttachmentId::from_bytes([id; 16]),
        key: Secret32::from_bytes([0x40 + id; 32]),
        name: format!("file{id}.png"),
        mime: "image/png".into(),
        size,
        hash: [id; 32],
        chunk_size,
        chunks,
    }
}

fn sample() -> Publication {
    Publication {
        page_id: PageId::from_bytes([0x11; 16]),
        title: "Title \u{e9}".into(),
        icon: Some("\u{1f30b}".into()),
        cover: Some("color:sand".into()),
        cover_y: 42,
        snapshot: b"KBD1 snapshot".to_vec(),
        attachments: vec![
            attachment(1, 10, 4, vec![chunk(1), chunk(2), chunk(3)]),
            attachment(2, 0, 4, vec![chunk(4)]),
        ],
        mentions: vec![PublishedPerson {
            account_id: AccountId::from_bytes([0x22; 16]),
            name: "Sigr\u{ed}\u{f0}ur".into(),
        }],
    }
}

fn link_key() -> LinkKey {
    LinkKey::from_secret(Secret32::from_bytes([0x77; 32]))
}

/// Byte offsets inside `encode_publication(&sample())`.
const TITLE_AT: usize = 17;
/// The cover position byte: 1 + 16 + (4 + 8) + (4 + 4) + (4 + 10).
const COVER_Y_AT: usize = 51;
/// The attachment count: the cover position, then the snapshot (4 + 13).
const ATTACHMENTS_AT: usize = COVER_Y_AT + 1 + 4 + 13;
/// The first attachment's chunk size: count, id, key, name, mime, size, hash.
const CHUNK_SIZE_AT: usize = ATTACHMENTS_AT + 4 + 16 + 32 + 4 + 9 + 4 + 9 + 8 + 32;

#[test]
fn the_plaintext_has_the_documented_layout_and_round_trips() {
    let publication = sample();
    let bytes = encode_publication(&publication).unwrap();
    let mut expected = vec![1u8];
    expected.extend_from_slice(&[0x11; 16]);
    let title = "Title \u{e9}".as_bytes();
    expected.extend_from_slice(&u32::try_from(title.len()).unwrap().to_le_bytes());
    expected.extend_from_slice(title);
    let icon = "\u{1f30b}".as_bytes();
    expected.extend_from_slice(&u32::try_from(icon.len()).unwrap().to_le_bytes());
    expected.extend_from_slice(icon);
    expected.extend_from_slice(&10u32.to_le_bytes());
    expected.extend_from_slice(b"color:sand");
    expected.push(42);
    expected.extend_from_slice(&13u32.to_le_bytes());
    expected.extend_from_slice(b"KBD1 snapshot");
    expected.extend_from_slice(&2u32.to_le_bytes());
    // The first attachment: id, key, name, mime, size, hash, chunk size, chunk count, chunk ids.
    expected.extend_from_slice(&[1; 16]);
    expected.extend_from_slice(&[0x41; 32]);
    expected.extend_from_slice(&9u32.to_le_bytes());
    expected.extend_from_slice(b"file1.png");
    expected.extend_from_slice(&9u32.to_le_bytes());
    expected.extend_from_slice(b"image/png");
    expected.extend_from_slice(&10u64.to_le_bytes());
    expected.extend_from_slice(&[1; 32]);
    expected.extend_from_slice(&4u32.to_le_bytes());
    expected.extend_from_slice(&3u32.to_le_bytes());
    for id in 1..=3u8 {
        expected.extend_from_slice(&[id; 32]);
    }
    assert_eq!(&bytes[..expected.len()], &expected[..]);
    assert_eq!(bytes[COVER_Y_AT], 42);
    assert_eq!(
        &bytes[CHUNK_SIZE_AT..CHUNK_SIZE_AT + 4],
        &4u32.to_le_bytes()
    );
    assert_eq!(decode_publication(&bytes).unwrap(), publication);
    // The mentions close the plaintext: count, account id, name.
    let name = "Sigr\u{ed}\u{f0}ur".len();
    let tail = bytes.len() - (4 + 16 + 4 + name);
    assert_eq!(&bytes[tail..tail + 4], &1u32.to_le_bytes());
    assert_eq!(&bytes[tail + 4..tail + 20], &[0x22; 16]);
}

#[test]
fn an_empty_icon_and_cover_read_back_as_none() {
    let publication = Publication {
        icon: None,
        cover: Some(String::new()),
        attachments: Vec::new(),
        mentions: Vec::new(),
        ..sample()
    };
    let decoded = decode_publication(&encode_publication(&publication).unwrap()).unwrap();
    assert_eq!(decoded.icon, None);
    assert_eq!(decoded.cover, None);
    // The smallest publication: no title, no snapshot, no attachments, no mentions.
    let minimal = Publication {
        title: String::new(),
        snapshot: Vec::new(),
        cover_y: 0,
        ..publication
    };
    let decoded = decode_publication(&encode_publication(&minimal).unwrap()).unwrap();
    assert_eq!(decoded.title, "");
    assert!(decoded.snapshot.is_empty());
}

#[test]
fn the_encoder_refuses_what_the_decoder_would() {
    let at_limit = Publication {
        title: "t".repeat(PUBLICATION_TITLE_MAX_BYTES),
        icon: Some("i".repeat(PUBLICATION_SHORT_MAX_BYTES)),
        cover: Some("c".repeat(PUBLICATION_SHORT_MAX_BYTES)),
        cover_y: 100,
        mentions: vec![
            PublishedPerson {
                account_id: AccountId::from_bytes([3; 16]),
                name: "n".repeat(PUBLICATION_SHORT_MAX_BYTES),
            };
            PUBLICATION_MAX_MENTIONS
        ],
        ..sample()
    };
    assert_eq!(
        decode_publication(&encode_publication(&at_limit).unwrap()).unwrap(),
        at_limit
    );
    let refused = |publication: Publication| {
        assert!(
            matches!(
                encode_publication(&publication),
                Err(SyncError::Protocol(_))
            ),
            "{publication:?}"
        );
    };
    refused(Publication {
        title: "t".repeat(PUBLICATION_TITLE_MAX_BYTES + 1),
        ..sample()
    });
    refused(Publication {
        icon: Some("i".repeat(PUBLICATION_SHORT_MAX_BYTES + 1)),
        ..sample()
    });
    refused(Publication {
        cover: Some("c".repeat(PUBLICATION_SHORT_MAX_BYTES + 1)),
        ..sample()
    });
    refused(Publication {
        cover_y: 101,
        ..sample()
    });
    let mut long_name = sample();
    long_name.attachments[0].name = "n".repeat(PUBLICATION_SHORT_MAX_BYTES + 1);
    refused(long_name);
    let mut long_mime = sample();
    long_mime.attachments[0].mime = "m".repeat(PUBLICATION_SHORT_MAX_BYTES + 1);
    refused(long_mime);
    let mut wrong_count = sample();
    wrong_count.attachments[0].chunks.pop();
    refused(wrong_count);
    let mut zero_chunk_size = sample();
    zero_chunk_size.attachments[1].chunk_size = 0;
    refused(zero_chunk_size);
    refused(Publication {
        mentions: vec![
            PublishedPerson {
                account_id: AccountId::from_bytes([3; 16]),
                name: String::new(),
            };
            PUBLICATION_MAX_MENTIONS + 1
        ],
        ..sample()
    });
    refused(Publication {
        attachments: vec![attachment(9, 0, 4, vec![chunk(9)]); LINK_MAX_CHUNKS + 1],
        ..sample()
    });
}

/// A valid plaintext with `bytes` written at `offset`.
fn corrupt(offset: usize, bytes: &[u8]) -> Vec<u8> {
    let mut out = encode_publication(&sample()).unwrap().to_vec();
    out[offset..offset + bytes.len()].copy_from_slice(bytes);
    out
}

#[test]
fn the_decoder_refuses_malformed_plaintext_with_encoding_errors() {
    let good = encode_publication(&sample()).unwrap().to_vec();
    // Every truncation, and a trailing byte.
    for len in 0..good.len() {
        assert!(
            matches!(
                decode_publication(&good[..len]),
                Err(SyncError::Encoding(_))
            ),
            "{len}"
        );
    }
    let mut longer = good.clone();
    longer.push(0);
    assert!(matches!(
        decode_publication(&longer),
        Err(SyncError::Encoding(_))
    ));
    // Another version, text that is not UTF-8 (the title ends in a lone lead byte) and a cover
    // position over 100.
    for bad in [
        corrupt(0, &[2]),
        corrupt(0, &[0]),
        corrupt(TITLE_AT + 4 + 7, &[0xc3]),
        corrupt(COVER_Y_AT, &[101]),
    ] {
        assert!(matches!(
            decode_publication(&bad),
            Err(SyncError::Encoding(_))
        ));
    }
    // A chunk size of zero, and one that makes the manifest list the wrong number of chunks
    // (size 10 in chunks of 4 is three; in chunks of 5 it is three too; of 3 it is four).
    assert!(decode_publication(&corrupt(CHUNK_SIZE_AT, &0u32.to_le_bytes())).is_err());
    assert!(decode_publication(&corrupt(CHUNK_SIZE_AT, &5u32.to_le_bytes())).is_ok());
    assert!(decode_publication(&corrupt(CHUNK_SIZE_AT, &3u32.to_le_bytes())).is_err());
    // A count the input cannot hold is refused without allocating, whatever it claims.
    assert_eq!(
        &good[ATTACHMENTS_AT..ATTACHMENTS_AT + 4],
        &2u32.to_le_bytes()
    );
    for claimed in [3u32, 1_000, u32::MAX] {
        assert!(decode_publication(&corrupt(ATTACHMENTS_AT, &claimed.to_le_bytes())).is_err());
    }
    let over = u32::try_from(LINK_MAX_CHUNKS + 1).unwrap();
    assert!(matches!(
        decode_publication(&corrupt(ATTACHMENTS_AT, &over.to_le_bytes())),
        Err(SyncError::Encoding(_))
    ));
    // The mention count: over the input and over the limit.
    let mentions_at = good.len() - (4 + 16 + 4 + "Sigr\u{ed}\u{f0}ur".len());
    let over = u32::try_from(PUBLICATION_MAX_MENTIONS + 1).unwrap();
    for claimed in [2u32, u32::MAX, over] {
        assert!(decode_publication(&corrupt(mentions_at, &claimed.to_le_bytes())).is_err());
    }
    // A title length over the limit.
    let over_title = u32::try_from(PUBLICATION_TITLE_MAX_BYTES + 1).unwrap();
    assert!(decode_publication(&corrupt(TITLE_AT, &over_title.to_le_bytes())).is_err());
}

#[test]
fn a_sealed_publication_opens_with_its_key_and_only_with_it() {
    let key = link_key();
    let publication = sample();
    let sealed = seal_publication(&key, &publication, &SeededRng::new(11)).unwrap();
    assert_eq!(open_publication(&key, &sealed).unwrap(), publication);
    // Kind 11, the header of the link, no signature, no bucket padding.
    assert_eq!(&sealed[..3], &[1, 1, 11]);
    assert_eq!(
        peek_publication(&sealed).unwrap(),
        PublicationHeader {
            link_id: key.link_id()
        }
    );
    let plaintext = encode_publication(&publication).unwrap();
    assert_eq!(
        sealed.len(),
        7 + PUBLICATION_HEADER_LEN + 24 + 4 + plaintext.len() + 16
    );
    // The page text and the attachment key are not in the sealed bytes.
    let contains = |needle: &[u8]| sealed.windows(needle.len()).any(|window| window == needle);
    assert!(!contains(b"Title"));
    assert!(!contains(b"KBD1 snapshot"));
    assert!(!contains(&[0x41; 32]));
    // Another key derives another link id, so it does not open the publication.
    let other = LinkKey::from_secret(Secret32::from_bytes([0x78; 32]));
    assert_eq!(
        open_publication(&other, &sealed).unwrap_err(),
        SyncError::Authentication
    );
    // Sealing twice gives two different envelopes (fresh nonces) with the same content.
    let again = seal_publication(&key, &publication, &SeededRng::new(12)).unwrap();
    assert_ne!(again, sealed);
    assert_eq!(open_publication(&key, &again).unwrap(), publication);
}

#[test]
fn a_tampered_publication_is_refused_and_a_moved_one_does_not_open() {
    let key = link_key();
    let sealed = seal_publication(&key, &sample(), &SeededRng::new(13)).unwrap();
    // A flipped bit anywhere is an error, never a different publication; so is any truncation.
    for index in 0..sealed.len() {
        let mut bad = sealed.clone();
        bad[index] ^= 0x01;
        assert!(open_publication(&key, &bad).is_err(), "byte {index}");
    }
    for len in 0..sealed.len() {
        assert!(open_publication(&key, &sealed[..len]).is_err(), "{len}");
    }
    // A publication whose header names another link than the key derives, sealed under the right
    // key by hand.
    let moved = seal_envelope(
        EnvelopeKind::Publication,
        &encode_publication_header(&PublicationHeader { link_id: link(9) }),
        &key.publication_key(),
        &encode_publication(&sample()).unwrap(),
        None,
        &SeededRng::new(14),
    );
    assert_eq!(
        open_publication(&key, &moved).unwrap_err(),
        SyncError::Authentication
    );
    // The same body under another header does not authenticate either (the header is AAD).
    let mut rebuilt = kb_crypto::Envelope::decode(&sealed).unwrap();
    rebuilt.header = encode_publication_header(&PublicationHeader {
        link_id: key.link_id(),
    });
    rebuilt.header[1] ^= 1;
    assert!(open_publication(&key, &rebuilt.encode()).is_err());
    // A signed publication, another envelope kind and a plaintext that is no publication.
    let header = encode_publication_header(&PublicationHeader {
        link_id: key.link_id(),
    });
    let plaintext = encode_publication(&sample()).unwrap();
    let signer = SigningKey::from_seed(&Secret32::from_bytes([8; 32]));
    let signed = seal_envelope(
        EnvelopeKind::Publication,
        &header,
        &key.publication_key(),
        &plaintext,
        Some(&signer),
        &SeededRng::new(15),
    );
    assert!(matches!(
        open_publication(&key, &signed),
        Err(SyncError::Encoding(_))
    ));
    let other_kind = seal_envelope(
        EnvelopeKind::Presence,
        &header,
        &key.publication_key(),
        &plaintext,
        None,
        &SeededRng::new(16),
    );
    assert!(matches!(
        open_publication(&key, &other_kind),
        Err(SyncError::Encoding(_))
    ));
    let junk = seal_envelope(
        EnvelopeKind::Publication,
        &header,
        &key.publication_key(),
        b"not a publication",
        None,
        &SeededRng::new(17),
    );
    assert!(matches!(
        open_publication(&key, &junk),
        Err(SyncError::Encoding(_))
    ));
}

#[test]
fn the_chunk_ids_of_an_opened_publication_are_the_declared_hashes() {
    let sealed = seal_publication(&link_key(), &sample(), &SeededRng::new(18)).unwrap();
    let opened = open_publication(&link_key(), &sealed).unwrap();
    assert_eq!(
        opened.chunk_ids(),
        vec![chunk(1), chunk(2), chunk(3), chunk(4)]
    );
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]

    #[test]
    fn arbitrary_bytes_never_panic_the_decoders(
        bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..512),
    ) {
        let _ = decode_publication(&bytes);
        let _ = open_publication(&link_key(), &bytes);
        let _ = peek_publication(&bytes);
    }

    #[test]
    fn a_publication_round_trips(
        title in "\\PC{0,40}",
        cover_y in 0u8..=100,
        snapshot in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..64),
        sizes in proptest::collection::vec((0u64..40, 1u32..9), 0..4),
        names in proptest::collection::vec("\\PC{0,12}", 0..4),
    ) {
        let attachments: Vec<PublishedAttachment> = sizes
            .iter()
            .enumerate()
            .map(|(index, (size, chunk_size))| {
                let count = usize::try_from(size / u64::from(*chunk_size) + 1).unwrap();
                let id = u8::try_from(index).unwrap() + 1;
                attachment(id, *size, *chunk_size, vec![chunk(id); count])
            })
            .collect();
        let publication = Publication {
            title,
            cover_y,
            snapshot,
            attachments,
            mentions: names
                .into_iter()
                .map(|name| PublishedPerson { account_id: AccountId::from_bytes([5; 16]), name })
                .collect(),
            ..sample()
        };
        let sealed = seal_publication(&link_key(), &publication, &SeededRng::new(21)).unwrap();
        proptest::prop_assert_eq!(open_publication(&link_key(), &sealed).unwrap(), publication);
    }
}
