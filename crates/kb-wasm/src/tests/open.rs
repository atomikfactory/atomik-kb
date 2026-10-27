//! Opening a publication sealed by `kb-sync`'s codec and decrypting the chunks `kb-crypto`'s stream sealer
//! makes, natively (the `#[wasm_bindgen]` functions are one-line wrappers over `open` and `decrypt`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::{Secret32, StreamSealer};
use kb_sync::v2::{PublishedAttachment, seal_publication};
use kb_types::testing::SeededRng;
use kb_types::{AttachmentId, ChunkId};

use super::{RAMP_FRAGMENT, publication};
use crate::*;

fn ramp_key() -> LinkKey {
    LinkKey::from_fragment(RAMP_FRAGMENT).unwrap()
}

fn attachment_id() -> AttachmentId {
    AttachmentId::from_bytes([2; 16])
}

fn stream_sealer(key: &Secret32) -> StreamSealer {
    let mut prefix = ATTACHMENT_AAD_DOMAIN.to_vec();
    prefix.extend_from_slice(attachment_id().as_bytes());
    StreamSealer::new(key.clone(), prefix)
}

/// A publication whose attachment is `plain` in chunks of `chunk_size`, made the way `kb-core` makes them (the
/// final chunk padded: a `u32` length, the bytes, random padding), with the sealed chunks.
fn publication_with_attachment(plain: &[u8], chunk_size: usize) -> (Publication, Vec<Vec<u8>>) {
    let key = Secret32::from_bytes([0x5a; 32]);
    let mut sealer = stream_sealer(&key);
    let rng = SeededRng::new(9);
    let full = plain.len() / chunk_size;
    let mut sealed = Vec::new();
    for index in 0..full {
        let part = &plain[index * chunk_size..(index + 1) * chunk_size];
        sealed.push(sealer.seal_chunk(part, false, &rng).unwrap());
    }
    let rest = &plain[full * chunk_size..];
    let mut last = u32::try_from(rest.len()).unwrap().to_le_bytes().to_vec();
    last.extend_from_slice(rest);
    last.extend_from_slice(&[0xee; 13]);
    sealed.push(sealer.seal_chunk(&last, true, &rng).unwrap());

    let mut publication = publication();
    publication.attachments = vec![PublishedAttachment {
        attachment_id: attachment_id(),
        key,
        name: "a.bin".into(),
        mime: "application/octet-stream".into(),
        size: plain.len() as u64,
        hash: kb_crypto::hash(plain),
        chunk_size: u32::try_from(chunk_size).unwrap(),
        chunks: sealed
            .iter()
            .map(|chunk| ChunkId::from_bytes(kb_crypto::hash(chunk)))
            .collect(),
    }];
    (publication, sealed)
}

#[test]
fn a_sealed_publication_opens_under_its_fragment() {
    let sealed = seal_publication(&ramp_key(), &publication(), &SeededRng::new(1)).unwrap();
    for fragment in [RAMP_FRAGMENT.to_owned(), format!("#{RAMP_FRAGMENT}")] {
        let opened = open(&fragment, &sealed).unwrap();
        assert_eq!(opened.title(), "A page");
        assert_eq!(opened.icon().as_deref(), Some("book"));
        assert_eq!(opened.cover_y(), 40);
        assert_eq!(opened.snapshot(), vec![0x4b, 0x42, 0x44, 0x31, 9, 9]);
        assert_eq!(opened.mentions_value()[0]["name"], "Bob");
        assert_eq!(opened.attachments_value()[0]["name"], "a.png");
    }
}

#[test]
fn a_publication_does_not_open_under_another_key_or_when_altered() {
    let sealed = seal_publication(&ramp_key(), &publication(), &SeededRng::new(1)).unwrap();

    // Another link's key: the header names another link, or the tag fails.
    let other = LinkKey::from_secret(Secret32::from_bytes([0x33; 32])).to_fragment();
    assert_eq!(
        open(&other, &sealed).err(),
        Some(ViewerError::Authentication)
    );
    // A fragment that is not a key never reaches the bytes.
    assert_eq!(open("not a key", &sealed).err(), Some(ViewerError::BadKey));

    // One flipped byte of the ciphertext fails the tag; the text never repeats the fragment or the bytes.
    let mut altered = sealed.clone();
    let last = altered.len() - 1;
    altered[last] ^= 1;
    let error = open(RAMP_FRAGMENT, &altered).err().unwrap();
    assert_eq!(error, ViewerError::Authentication);
    assert!(!error.to_string().contains(RAMP_FRAGMENT));

    // Bytes that are not an envelope, and a truncated one, are "malformed" or "authentication", never a panic.
    for bytes in [&[][..], &[1, 2, 3][..], &sealed[..sealed.len() / 2]] {
        let error = open(RAMP_FRAGMENT, bytes).err().unwrap();
        assert!(
            matches!(
                error,
                ViewerError::Malformed(_) | ViewerError::Authentication
            ),
            "{error}"
        );
    }
}

#[test]
fn a_publication_made_for_another_link_is_refused_at_its_own_id() {
    // The page asked for the link its fragment derives and the server answered with another link's publication.
    let other = LinkKey::from_secret(Secret32::from_bytes([0x44; 32]));
    let sealed = seal_publication(&other, &publication(), &SeededRng::new(2)).unwrap();
    assert_eq!(
        open(RAMP_FRAGMENT, &sealed).err(),
        Some(ViewerError::Authentication)
    );
    assert!(open(&other.to_fragment(), &sealed).is_ok());
}

#[test]
fn every_chunk_of_an_attachment_decrypts_to_its_bytes() {
    let plain = b"abcdefghi";
    let (publication, sealed) = publication_with_attachment(plain, 4);
    assert_eq!(sealed.len(), 3);
    let opened = OpenedPublication::new(publication);
    let id = attachment_id().to_string();

    let mut whole = Vec::new();
    for (index, chunk) in sealed.iter().enumerate() {
        let is_final = index == sealed.len() - 1;
        let bytes = opened
            .decrypt(&id, u32::try_from(index).unwrap(), is_final, chunk)
            .unwrap();
        whole.extend_from_slice(&bytes);
    }
    // The padding of the final chunk is gone: the chunks' answers concatenate to the attachment.
    assert_eq!(whole, plain);
}

#[test]
fn an_empty_attachment_has_one_final_chunk_with_no_bytes() {
    let (publication, sealed) = publication_with_attachment(b"", 4);
    assert_eq!(sealed.len(), 1);
    let opened = OpenedPublication::new(publication);
    let id = attachment_id().to_string();
    assert_eq!(
        opened.decrypt(&id, 0, true, &sealed[0]).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn a_chunk_that_is_not_the_one_listed_at_its_place_does_not_decrypt() {
    let (publication, sealed) = publication_with_attachment(b"abcdefghi", 4);
    let opened = OpenedPublication::new(publication);
    let id = attachment_id().to_string();

    // Reordered: chunk 1 offered as chunk 0 (its hash is not the one listed there).
    assert_eq!(
        opened.decrypt(&id, 0, false, &sealed[1]).unwrap_err(),
        ViewerError::Authentication
    );
    // The right bytes with the final flag the other way round fail the AAD.
    assert_eq!(
        opened.decrypt(&id, 0, true, &sealed[0]).unwrap_err(),
        ViewerError::Authentication
    );
    assert_eq!(
        opened.decrypt(&id, 2, false, &sealed[2]).unwrap_err(),
        ViewerError::Authentication
    );
    // A flipped byte changes the hash.
    let mut altered = sealed[0].clone();
    altered[30] ^= 1;
    assert_eq!(
        opened.decrypt(&id, 0, false, &altered).unwrap_err(),
        ViewerError::Authentication
    );
    // Nothing at all.
    assert_eq!(
        opened.decrypt(&id, 0, false, &[]).unwrap_err(),
        ViewerError::Authentication
    );
}

#[test]
fn an_index_past_the_end_and_an_unknown_attachment_are_told_apart() {
    let (publication, sealed) = publication_with_attachment(b"abcdefghi", 4);
    let opened = OpenedPublication::new(publication);
    let id = attachment_id().to_string();
    for index in [3, u32::MAX] {
        assert!(matches!(
            opened.decrypt(&id, index, true, &sealed[2]).unwrap_err(),
            ViewerError::Malformed(_)
        ));
    }
    let other = AttachmentId::from_bytes([3; 16]).to_string();
    for unknown in [other.as_str(), "not an id"] {
        assert_eq!(
            opened.decrypt(unknown, 0, false, &sealed[0]).unwrap_err(),
            ViewerError::UnknownAttachment
        );
    }
}

#[test]
fn a_final_chunk_with_a_damaged_padding_prefix_is_malformed() {
    // A final chunk that claims more bytes than it holds, sealed under the right key and position.
    let key = Secret32::from_bytes([0x5a; 32]);
    let mut body = 1000_u32.to_le_bytes().to_vec();
    body.extend_from_slice(b"short");
    let sealed = stream_sealer(&key)
        .seal_chunk(&body, true, &SeededRng::new(3))
        .unwrap();
    let mut publication = publication();
    publication.attachments[0].key = key;
    publication.attachments[0].chunks = vec![ChunkId::from_bytes(kb_crypto::hash(&sealed))];
    let opened = OpenedPublication::new(publication);
    assert!(matches!(
        opened
            .decrypt(&attachment_id().to_string(), 0, true, &sealed)
            .unwrap_err(),
        ViewerError::Malformed(_)
    ));
}

#[test]
fn the_viewers_fixtures_open_and_their_chunk_decrypts_to_the_picture() {
    // `apps/viewer/fixtures/` (made by `examples/viewer_fixture.rs`) is what the viewer's Vitest opens through the
    // built module: it must still open under today's codec.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/viewer/fixtures");
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    let description: serde_json::Value = serde_json::from_slice(&read("fixture.json")).unwrap();
    let fragment = description["fragment"].as_str().unwrap();
    assert_eq!(
        link_id_of(fragment).unwrap().to_string(),
        description["linkId"].as_str().unwrap()
    );
    for (file, title) in [
        ("publication-1.bin", &description["titles"][0]),
        ("publication-2.bin", &description["titles"][1]),
    ] {
        let opened = open(fragment, &read(file)).unwrap();
        assert_eq!(opened.title(), title.as_str().unwrap());
        assert_eq!(opened.page_id(), description["pageId"].as_str().unwrap());
        assert_eq!(&opened.snapshot()[..4], b"KBD1");
    }
    let opened = open(fragment, &read("publication-1.bin")).unwrap();
    let attachment = &description["attachment"];
    let chunk = read("chunk.bin");
    assert_eq!(
        attachment["chunks"][0].as_str().unwrap(),
        kb_crypto::hash(&chunk)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let picture = opened
        .decrypt(
            attachment["attachmentId"].as_str().unwrap(),
            0,
            true,
            &chunk,
        )
        .unwrap();
    assert_eq!(picture, read("picture.png"));
}

#[test]
fn the_frozen_publication_of_kb_sync_opens_through_the_modules_entry_point() {
    // `fixtures/sync/publication-v2.bin` is the golden file of the codec (key `[0x77; 32]`): the module opens what the
    // codec froze, names the page and its mentions as the vectors say, and lists the attachments without their keys.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sync/publication-v2.bin");
    let bytes = std::fs::read(path).unwrap();
    let fragment = LinkKey::from_secret(Secret32::from_bytes([0x77; 32])).to_fragment();
    let opened = open(&fragment, &bytes).unwrap();
    assert_eq!(opened.title(), "Trip to Reykjav\u{ed}k");
    assert_eq!(opened.icon().as_deref(), Some("\u{1f30b}"));
    assert_eq!(opened.cover_y(), 35);
    assert_eq!(opened.attachments_value().as_array().unwrap().len(), 2);
    assert_eq!(opened.mentions_value().as_array().unwrap().len(), 2);
    assert!(!opened.attachments_json().contains("5a5a"));
    // Under the key of another link it is refused, not mis-read.
    let other = LinkKey::from_secret(Secret32::from_bytes([0x78; 32])).to_fragment();
    assert_eq!(
        open(&other, &bytes).err(),
        Some(ViewerError::Authentication)
    );
}
