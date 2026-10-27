//! The link key, its derivations and its fragment form.

#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use kb_crypto::derive_key;
use kb_types::testing::SeededRng;

/// The key `00 01 .. 1f` of the cross-checked vectors.
fn ramp_key() -> LinkKey {
    LinkKey::from_secret(Secret32::from_bytes(core::array::from_fn(|i| {
        u8::try_from(i).unwrap()
    })))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_link_id_and_the_publication_key_are_the_documented_derivations() {
    let key = ramp_key();
    // Cross-checked against HMAC-SHA256 outside this crate (`kb-crypto`'s frozen label vectors).
    assert_eq!(
        hex(derive_key(key.secret(), Label::LINK_ID, b"").expose()),
        "1483df439bde69b52d0148d5a52d933b88c6e5759edd5b434307a13325461fc4"
    );
    assert_eq!(
        key.link_id().to_string(),
        "1483df43-9bde-69b5-2d01-48d5a52d933b",
        "the first 16 bytes of the link-id derivation, as the UUID text of every id"
    );
    assert_eq!(key.link_id(), link_id(key.secret()));
    assert_eq!(
        hex(key.publication_key().expose()),
        "2da85e96eb217d756e459f45f60bd32522124cba6220a392e6ea0ff0c996697e"
    );
    assert_eq!(key.publication_key(), publication_key(key.secret()));
}

#[test]
fn the_id_and_the_publication_key_are_unrelated_to_each_other_and_to_the_key() {
    let key = ramp_key();
    assert_ne!(
        &key.publication_key().expose()[..16],
        key.link_id().as_bytes(),
        "the server's id is not a prefix of the publication key"
    );
    assert_ne!(&key.publication_key(), key.secret());
    let other = LinkKey::from_secret(Secret32::from_bytes([9; 32]));
    assert_ne!(key.link_id(), other.link_id());
    assert_ne!(key.publication_key(), other.publication_key());
}

#[test]
fn the_fragment_is_base64url_without_padding() {
    assert_eq!(
        ramp_key().to_fragment(),
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
    );
    // The two symbols that differ from standard base64: `-` for `+` and `_` for `/`.
    let pattern = LinkKey::from_secret(Secret32::from_bytes(core::array::from_fn(|i| {
        if i % 2 == 0 { 0xfb } else { 0xff }
    })));
    assert_eq!(
        pattern.to_fragment(),
        "-__7__v_-__7__v_-__7__v_-__7__v_-__7__v_-_8"
    );
    let all_ones = LinkKey::from_secret(Secret32::from_bytes([0xff; 32]));
    assert_eq!(
        all_ones.to_fragment(),
        "__________________________________________8"
    );
    for key in [ramp_key(), pattern, all_ones] {
        assert_eq!(key.to_fragment().len(), LINK_KEY_FRAGMENT_LEN);
    }
}

#[test]
fn a_fragment_round_trips_with_or_without_the_hash() {
    let rng = SeededRng::new(11);
    for _ in 0..64 {
        let key = LinkKey::generate(&rng);
        let text = key.to_fragment();
        assert_eq!(LinkKey::from_fragment(&text).unwrap(), key);
        assert_eq!(LinkKey::from_fragment(&format!("#{text}")).unwrap(), key);
    }
    assert_ne!(
        LinkKey::generate(&rng),
        LinkKey::generate(&rng),
        "keys are random"
    );
}

#[test]
fn a_fragment_that_is_not_exactly_a_key_is_refused_without_echoing_it() {
    let good = ramp_key().to_fragment();
    let mut cases: Vec<String> = vec![
        String::new(),
        "#".to_owned(),
        good[..42].to_owned(),
        format!("{good}A"),
        format!("{good}="),
        good.replace('A', "+"),
        good.replace('A', "/"),
        good.replace('A', " "),
        format!("##{good}"),
        // 43 characters, but the last one has bits set that no key can have (a second spelling of
        // the key whose last character is `8`).
        format!("{}9", &good[..42]),
    ];
    // Standard base64 and a multibyte character are refused too.
    cases.push("é".repeat(LINK_KEY_FRAGMENT_LEN));
    for text in cases {
        let error = LinkKey::from_fragment(&text).unwrap_err();
        let KeysError::Corrupted(message) = error else {
            panic!("expected Corrupted for {text:?}");
        };
        assert!(
            text.len() < 4 || !message.contains(text.trim_start_matches('#')),
            "the message never repeats the text"
        );
    }
}

#[test]
fn the_debug_form_never_shows_the_key() {
    let key = ramp_key();
    let shown = format!("{key:?}");
    assert!(!shown.contains("AAECAw"), "{shown}");
    assert!(!shown.contains("00010203"), "{shown}");
}
