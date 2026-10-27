//! The module's logic, natively (the `#[wasm_bindgen]` functions are one-line wrappers over it): the link
//! id of a fragment, the getters and the JSON views here; opening a publication and decrypting chunks in
//! `open.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::Secret32;
use kb_sync::v2::{PublishedAttachment, PublishedPerson};
use kb_types::{AccountId, AttachmentId, ChunkId, PageId};

use super::*;

mod open;

/// The key `00 01 .. 1f` and its fragment (the vectors of `kb-keys`' `link` module).
const RAMP_FRAGMENT: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

fn publication() -> Publication {
    Publication {
        page_id: PageId::from_bytes([1; 16]),
        title: "A page".into(),
        icon: Some("book".into()),
        cover: None,
        cover_y: 40,
        snapshot: vec![0x4b, 0x42, 0x44, 0x31, 9, 9],
        attachments: vec![PublishedAttachment {
            attachment_id: AttachmentId::from_bytes([2; 16]),
            key: Secret32::from_bytes([0x5a; 32]),
            name: "a.png".into(),
            mime: "image/png".into(),
            size: 10,
            hash: [3; 32],
            chunk_size: 4 * 1024 * 1024,
            chunks: vec![ChunkId::from_bytes([4; 32])],
        }],
        mentions: vec![PublishedPerson {
            account_id: AccountId::from_bytes([5; 16]),
            name: "Bob".into(),
        }],
    }
}

#[test]
fn the_link_id_is_derived_from_the_fragment_with_or_without_the_hash() {
    let expected = "1483df43-9bde-69b5-2d01-48d5a52d933b";
    assert_eq!(link_id_of(RAMP_FRAGMENT).unwrap().to_string(), expected);
    assert_eq!(
        link_id_of(&format!("#{RAMP_FRAGMENT}"))
            .unwrap()
            .to_string(),
        expected
    );
}

#[test]
fn a_fragment_that_is_not_a_key_is_refused_without_echoing_it() {
    for text in [
        "",
        "#",
        "short",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9",
        "+AECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
    ] {
        let error = link_id_of(text).unwrap_err();
        assert_eq!(error, ViewerError::BadKey);
        assert!(error.to_string().starts_with("bad_key: "));
        assert!(text.len() < 4 || !error.to_string().contains(text));
    }
}

#[test]
fn the_error_messages_carry_their_code_first() {
    for (error, code) in [
        (ViewerError::BadKey, "bad_key: "),
        (ViewerError::Authentication, "authentication: "),
        (ViewerError::Malformed("x".into()), "malformed: "),
        (ViewerError::UnknownAttachment, "unknown_attachment: "),
    ] {
        assert!(error.to_string().starts_with(code), "{error}");
    }
}

#[test]
fn an_opened_publication_shows_its_page_and_keeps_the_keys_inside() {
    let opened = OpenedPublication::new(publication());
    assert_eq!(opened.page_id(), PageId::from_bytes([1; 16]).to_string());
    assert_eq!(opened.title(), "A page");
    assert_eq!(opened.icon().as_deref(), Some("book"));
    assert_eq!(opened.cover(), None);
    assert_eq!(opened.cover_y(), 40);
    assert_eq!(opened.snapshot(), vec![0x4b, 0x42, 0x44, 0x31, 9, 9]);

    let attachments: serde_json::Value = serde_json::from_str(&opened.attachments_json()).unwrap();
    assert_eq!(
        attachments,
        serde_json::json!([{
            "attachmentId": AttachmentId::from_bytes([2; 16]).to_string(),
            "name": "a.png",
            "mime": "image/png",
            "size": 10,
            "hash": "03".repeat(32),
            "chunkSize": 4 * 1024 * 1024,
            "chunks": ["04".repeat(32)],
        }])
    );
    let mentions: serde_json::Value = serde_json::from_str(&opened.mentions_json()).unwrap();
    assert_eq!(
        mentions,
        serde_json::json!([{ "accountId": AccountId::from_bytes([5; 16]).to_string(), "name": "Bob" }])
    );
    // No key, in any spelling, leaves through a getter or a JSON view.
    let everything = format!("{}{}", opened.attachments_json(), opened.mentions_json());
    assert!(!everything.contains("5a5a") && !everything.contains("\"key\""));
}
