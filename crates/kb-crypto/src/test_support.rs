//! Helpers shared by the unit tests: hex conversion and the known-answer files under
//! `fixtures/crypto/`, embedded at compile time with `include_str!`.

#![allow(clippy::unwrap_used, clippy::panic)]

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses lowercase or uppercase hex.
pub(crate) fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0, "odd number of hex digits");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// Exactly 32 bytes of hex.
pub(crate) fn unhex32(text: &str) -> [u8; 32] {
    unhex(text).try_into().unwrap()
}

/// A known-answer file from `fixtures/crypto/`, parsed.
pub(crate) fn kat(name: &str) -> serde_json::Value {
    let text = match name {
        "kat-xchacha20poly1305.json" => {
            include_str!("../../../fixtures/crypto/kat-xchacha20poly1305.json")
        }
        "kat-ed25519.json" => include_str!("../../../fixtures/crypto/kat-ed25519.json"),
        "kat-x25519.json" => include_str!("../../../fixtures/crypto/kat-x25519.json"),
        "kat-argon2id.json" => include_str!("../../../fixtures/crypto/kat-argon2id.json"),
        "kat-bip39.json" => include_str!("../../../fixtures/crypto/kat-bip39.json"),
        other => panic!("no known-answer file named {other}"),
    };
    let file: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(
        file["source"].as_str().is_some_and(|s| !s.is_empty()),
        "{name} names its source"
    );
    file
}
