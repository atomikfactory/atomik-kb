//! Makes the fixtures of the web viewer's tests under `apps/viewer/fixtures/` (M13 slice 4): a publication of a
//! page with a heading, a mention of a person, bold text and a picture (which is also the page's cover and
//! icon-less header picture), sealed by `kb-sync`'s codec under a fixed link key, a second one of the same link
//! with one more paragraph (what a poll finds after an edit), and the picture's one sealed chunk, made the way
//! `kb-core` makes an attachment (the final chunk padded). The viewer's Vitest opens them through the built
//! `kb-wasm` and imports their snapshots with `loro-crdt`; `kb-wasm`'s own tests open them natively.
//!
//! Every input is fixed here and every random byte comes from `kb_types::testing::SeededRng`, so the output is
//! reproducible. Run it only when a format changes on purpose:
//!
//! ```text
//! cargo run -p kb-wasm --example viewer_fixture
//! ```
//!
//! **Existing files are never rewritten**: a file whose bytes differ is reported and kept. To regenerate one on
//! purpose, delete it first. `fixtures/picture.png` is the input and is not made here.

// A developer tool, not library code: printing and unwrapping are fine here.
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use kb_crypto::{CHUNK_SIZE, Secret32, StreamSealer, bucket_size};
use kb_doc::{Doc, DocKind};
use kb_model::PageDoc;
use kb_sync::v2::{LinkKey, Publication, PublishedAttachment, PublishedPerson, seal_publication};
use kb_types::attachments::ATTACHMENT_AAD_DOMAIN;
use kb_types::testing::SeededRng;
use kb_types::{AccountId, AttachmentId, BlockId, ChunkId, PageId};
use serde_json::{Value, json};

/// The key `00 01 .. 1f`: the fragment `AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8`.
fn link_key() -> LinkKey {
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::try_from(index).unwrap();
    }
    LinkKey::from_secret(Secret32::from_bytes(bytes))
}

fn block(n: u8) -> String {
    BlockId::from_random_bytes([n; 16]).to_string()
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

/// The page's first revision.
fn page_json(attachment: AttachmentId, bob: AccountId) -> Value {
    json!({
        "type": "doc",
        "content": [
            { "type": "heading", "attrs": { "id": block(1), "level": 2 }, "content": [text("Packing list")] },
            { "type": "paragraph", "attrs": { "id": block(2) }, "content": [
                text("Ask "),
                { "type": "personMention", "attrs": { "accountId": bob.to_string(), "label": "Bob" } },
                text(" to bring the "),
                { "type": "text", "text": "tickets", "marks": [{ "type": "bold" }] },
                text("."),
            ] },
            { "type": "image", "attrs": { "id": block(3), "attachmentId": attachment.to_string(), "alt": "A striped picture" } },
        ]
    })
}

/// Pads the final chunk as `kb-core` does: a `u32` length, the bytes, random bytes up to the size bucket.
fn pad_final(plain: &[u8]) -> Vec<u8> {
    let min = plain.len() + 4;
    let target = bucket_size(min).min(CHUNK_SIZE + 4).max(min);
    let mut out = Vec::with_capacity(target);
    out.extend_from_slice(&u32::try_from(plain.len()).unwrap().to_le_bytes());
    out.extend_from_slice(plain);
    out.resize(target, 0xa5);
    out
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/viewer/fixtures");
    let picture = std::fs::read(dir.join("picture.png")).expect("apps/viewer/fixtures/picture.png");

    let page_id = PageId::from_bytes([0x11; 16]);
    let attachment_id = AttachmentId::from_bytes([0x22; 16]);
    let bob = AccountId::from_bytes([0x33; 16]);
    let attachment_key = Secret32::from_bytes([0x5a; 32]);
    let rng = SeededRng::new(7);

    // The picture's only chunk.
    let mut prefix = ATTACHMENT_AAD_DOMAIN.to_vec();
    prefix.extend_from_slice(attachment_id.as_bytes());
    let sealed_chunk = StreamSealer::new(attachment_key.clone(), prefix)
        .seal_chunk(&pad_final(&picture), true, &rng)
        .unwrap();
    let chunk_id = ChunkId::from_bytes(kb_crypto::hash(&sealed_chunk));

    // The two revisions of the page's document.
    let page =
        PageDoc::from_portable(Doc::new(DocKind::Page, 7), &page_json(attachment_id, bob)).unwrap();
    let snapshot_1 = page.doc().export_shallow_snapshot().unwrap();
    page.append_portable(&json!({ "type": "doc", "content": [
        { "type": "paragraph", "attrs": { "id": block(4) }, "content": [text("Added after the first publication.")] }
    ] }))
    .unwrap();
    let snapshot_2 = page.doc().export_shallow_snapshot().unwrap();

    let publication = |title: &str, snapshot: Vec<u8>| Publication {
        page_id,
        title: title.to_owned(),
        icon: Some("🧳".to_owned()),
        cover: Some(format!("attachment:{attachment_id}")),
        cover_y: 30,
        snapshot,
        attachments: vec![PublishedAttachment {
            attachment_id,
            key: attachment_key.clone(),
            name: "picture.png".to_owned(),
            mime: "image/png".to_owned(),
            size: picture.len() as u64,
            hash: kb_crypto::hash(&picture),
            chunk_size: u32::try_from(CHUNK_SIZE).unwrap(),
            chunks: vec![chunk_id],
        }],
        mentions: vec![PublishedPerson {
            account_id: bob,
            name: "Bob".to_owned(),
        }],
    };
    let key = link_key();
    let first = seal_publication(&key, &publication("Trip notes", snapshot_1), &rng).unwrap();
    let second =
        seal_publication(&key, &publication("Trip notes (updated)", snapshot_2), &rng).unwrap();

    let description = json!({
        "fragment": key.to_fragment(),
        "linkId": key.link_id().to_string(),
        "pageId": page_id.to_string(),
        "titles": ["Trip notes", "Trip notes (updated)"],
        "icon": "🧳",
        "cover": format!("attachment:{attachment_id}"),
        "coverY": 30,
        "mentions": [{ "accountId": bob.to_string(), "name": "Bob" }],
        "attachment": {
            "attachmentId": attachment_id.to_string(),
            "name": "picture.png",
            "mime": "image/png",
            "size": picture.len(),
            "chunkSize": CHUNK_SIZE,
            "chunks": [chunk_id.to_hex()],
        },
        "texts": ["Packing list", "Ask @Bob to bring the tickets.", "Added after the first publication."],
    });
    let description = format!("{}\n", serde_json::to_string_pretty(&description).unwrap());

    let outputs: Vec<(String, Vec<u8>)> = vec![
        ("publication-1.bin".to_owned(), first),
        ("publication-2.bin".to_owned(), second),
        ("chunk.bin".to_owned(), sealed_chunk),
        ("fixture.json".to_owned(), description.into_bytes()),
    ];
    for (name, bytes) in outputs {
        let path = dir.join(name);
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => {
                println!("unchanged {} ({} bytes)", path.display(), bytes.len());
            }
            Ok(_) => {
                println!(
                    "KEPT {}: the frozen bytes differ from today's output (delete it to regenerate)",
                    path.display()
                );
            }
            Err(_) => {
                std::fs::write(&path, &bytes).unwrap();
                println!("wrote {} ({} bytes)", path.display(), bytes.len());
            }
        }
    }
}
