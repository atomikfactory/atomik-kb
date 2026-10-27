//! The JSON half of the wire (ADR-0021 §5-6, ADR-0045): metadata travels as camelCase JSON with
//! binary fields in hex (nonces, epochs, hashes, signatures) or base64 (device log
//! entries, heads); payloads (entries, snapshots, chunks, pages) are raw
//! `application/octet-stream` bodies and never pass through here.
//!
//! The encoders are written out instead of pulling in crates: they are a few lines each
//! and the decoders refuse anything that is not exactly what the protocol says (a hex
//! field of the wrong length is an `Encoding` error, not a silently truncated value).

use kb_sync::SyncError;
use kb_types::{AccountId, DeviceId};
use serde::{Deserialize, Serialize};

// ----- hex --------------------------------------------------------------------------------

/// Lowercase hex.
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes hex (either case) into exactly `N` bytes.
pub fn hex_decode<const N: usize>(field: &str, text: &str) -> Result<[u8; N], SyncError> {
    let bad = || SyncError::Encoding(format!("{field} is not {N} bytes of hex"));
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return Err(bad());
    }
    let mut out = [0u8; N];
    for (slot, &[high, low]) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
        let high = hex_digit(high).ok_or_else(bad)?;
        let low = hex_digit(low).ok_or_else(bad)?;
        *slot = (high << 4) | low;
    }
    Ok(out)
}

// ----- base64 -------------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (RFC 4648 §4) with padding.
#[must_use]
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let sextet = |shift: u32| char::from(B64[((triple >> shift) & 0x3f) as usize]);
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}

fn base64_value(c: u8) -> Option<u32> {
    let value = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// Decodes base64: the standard or the URL-safe alphabet, padded or not (a server may
/// pick either; the bytes are what matters). Anything else is an `Encoding` error.
pub fn base64_decode(field: &str, text: &str) -> Result<Vec<u8>, SyncError> {
    let bad = || SyncError::Encoding(format!("{field} is not base64"));
    let trimmed = text.trim_end_matches('=');
    if text.len() - trimmed.len() > 2 {
        return Err(bad());
    }
    let digits = trimmed.as_bytes();
    if digits.len() % 4 == 1 {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= base64_value(c).ok_or_else(bad)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Ok(out)
}

// ----- request and response bodies ------------------------------------------------------------

/// `POST /v2/auth/challenge` (body).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeRequestJson {
    pub account_id: AccountId,
    pub device_id: DeviceId,
}

/// `POST /v2/auth/challenge` (answer). The epoch is optional: without it the client asks
/// `GET /v2/server`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeJson {
    pub nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_epoch: Option<String>,
}

/// `POST /v2/auth/session` (body).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequestJson {
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub nonce: String,
    pub signature: String,
}

/// `POST /v2/auth/session` (answer). `expiresAt` is informational: a 401 is what makes
/// the client authenticate again.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionJson {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

// ----- pairing mailboxes and recovery (ADR-0022) ----------------------------------------------

/// A pairing role on the wire: `"joiner"` or `"sponsor"` (`0` and `1` are read too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RoleJson {
    Name(RoleName),
    Number(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleName {
    Joiner,
    Sponsor,
}

impl RoleJson {
    /// The wire form of a `kb_sync` role byte; an unknown byte is `Protocol`.
    pub fn from_byte(role: u8) -> Result<Self, SyncError> {
        match role {
            kb_sync::PAIRING_ROLE_JOINER => Ok(Self::Name(RoleName::Joiner)),
            kb_sync::PAIRING_ROLE_SPONSOR => Ok(Self::Name(RoleName::Sponsor)),
            other => Err(SyncError::Protocol(format!("unknown pairing role {other}"))),
        }
    }

    /// The `kb_sync` role byte.
    pub fn to_byte(self) -> Result<u8, SyncError> {
        match self {
            Self::Name(RoleName::Joiner) => Ok(kb_sync::PAIRING_ROLE_JOINER),
            Self::Name(RoleName::Sponsor) => Ok(kb_sync::PAIRING_ROLE_SPONSOR),
            Self::Number(byte @ (kb_sync::PAIRING_ROLE_JOINER | kb_sync::PAIRING_ROLE_SPONSOR)) => {
                Ok(byte)
            }
            Self::Number(other) => {
                Err(SyncError::Encoding(format!("unknown pairing role {other}")))
            }
        }
    }
}

/// `POST /v2/pair` (body).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairCreateJson {
    /// The joiner's clear offer, base64.
    pub offer: String,
}

/// `POST /v2/pair` (answer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairCreatedJson {
    /// 12 hex characters.
    pub mailbox_id: String,
    /// Unix ms, server clock.
    pub expires_at: i64,
}

/// `POST /v2/pair/{id}/messages` (body).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairPostJson {
    pub role: RoleJson,
    /// Base64.
    pub bytes: String,
}

/// One message of `GET /v2/pair/{id}/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairMessageJson {
    pub seq: u32,
    pub role: RoleJson,
    /// Base64.
    pub bytes: String,
}

/// `GET /v2/pair/{id}/messages` (answer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairPageJson {
    #[serde(default)]
    pub messages: Vec<PairMessageJson>,
    pub expires_at: i64,
    #[serde(default)]
    pub closed: bool,
}

impl PairPageJson {
    pub fn into_page(self) -> Result<kb_sync::PairingPage, SyncError> {
        let messages = self
            .messages
            .into_iter()
            .map(|message| {
                Ok(kb_sync::PairingMessage {
                    seq: message.seq,
                    role: message.role.to_byte()?,
                    bytes: base64_decode("bytes", &message.bytes)?,
                })
            })
            .collect::<Result<Vec<_>, SyncError>>()?;
        Ok(kb_sync::PairingPage {
            messages,
            expires_at: self.expires_at,
            closed: self.closed,
        })
    }
}

/// An error answer: `{"error": code, "message": text}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorJson {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_refuses_wrong_lengths_and_digits() {
        let bytes: [u8; 4] = [0x00, 0xab, 0x10, 0xff];
        assert_eq!(hex_encode(&bytes), "00ab10ff");
        assert_eq!(hex_decode::<4>("x", "00ab10ff").unwrap(), bytes);
        assert_eq!(hex_decode::<4>("x", "00AB10FF").unwrap(), bytes);
        assert!(hex_decode::<4>("x", "00ab10f").is_err());
        assert!(hex_decode::<4>("x", "00ab10ff00").is_err());
        assert!(hex_decode::<4>("x", "00ab10fg").is_err());
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), encoded);
            assert_eq!(base64_decode("x", encoded).unwrap(), plain.as_bytes());
            // Unpadded is accepted too.
            assert_eq!(
                base64_decode("x", encoded.trim_end_matches('=')).unwrap(),
                plain.as_bytes()
            );
        }
    }

    #[test]
    fn base64_accepts_both_alphabets_and_refuses_garbage() {
        let bytes: Vec<u8> = (0..=255).collect();
        let standard = base64_encode(&bytes);
        let url_safe = standard.replace('+', "-").replace('/', "_");
        assert_eq!(base64_decode("x", &standard).unwrap(), bytes);
        assert_eq!(base64_decode("x", &url_safe).unwrap(), bytes);
        assert!(base64_decode("x", "Zm9v!").is_err());
        assert!(base64_decode("x", "Z").is_err());
        assert!(base64_decode("x", "Zg===").is_err());
    }

    #[test]
    fn json_bodies_are_camel_case() {
        let account = AccountId::from_bytes([1; 16]);
        let device = DeviceId::from_bytes([2; 16]);
        let value = serde_json::to_value(SessionRequestJson {
            account_id: account,
            device_id: device,
            nonce: "aa".into(),
            signature: "bb".into(),
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "accountId": "01010101-0101-0101-0101-010101010101",
                "deviceId": "02020202-0202-0202-0202-020202020202",
                "nonce": "aa",
                "signature": "bb",
            })
        );
    }

    #[test]
    fn pairing_roles_travel_as_names_and_numbers_are_read() {
        assert_eq!(
            serde_json::to_value(RoleJson::from_byte(0).unwrap()).unwrap(),
            serde_json::json!("joiner")
        );
        assert_eq!(
            serde_json::to_value(RoleJson::from_byte(1).unwrap()).unwrap(),
            serde_json::json!("sponsor")
        );
        assert!(RoleJson::from_byte(2).is_err());
        for (text, byte) in [(r#""joiner""#, 0), (r#""sponsor""#, 1), ("0", 0), ("1", 1)] {
            let role: RoleJson = serde_json::from_str(text).unwrap();
            assert_eq!(role.to_byte().unwrap(), byte, "{text}");
        }
        let unknown: RoleJson = serde_json::from_str("7").unwrap();
        assert!(unknown.to_byte().is_err());
        assert!(serde_json::from_str::<RoleJson>(r#""admin""#).is_err());
    }

    #[test]
    fn a_pairing_page_decodes_its_messages() {
        let page: PairPageJson = serde_json::from_str(
            r#"{"messages":[{"seq":0,"role":"joiner","bytes":"AQID"},{"seq":1,"role":1,"bytes":""}],"expiresAt":1700000000000,"closed":true}"#,
        )
        .unwrap();
        let page = page.into_page().unwrap();
        assert_eq!(page.expires_at, 1_700_000_000_000);
        assert!(page.closed);
        assert_eq!(
            page.messages,
            [
                kb_sync::PairingMessage {
                    seq: 0,
                    role: 0,
                    bytes: vec![1, 2, 3]
                },
                kb_sync::PairingMessage {
                    seq: 1,
                    role: 1,
                    bytes: Vec::new()
                },
            ]
        );
        // `closed` and `messages` may be left out.
        let empty: PairPageJson = serde_json::from_str(r#"{"expiresAt":5}"#).unwrap();
        assert!(empty.into_page().unwrap().messages.is_empty());
    }
}
