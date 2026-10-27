//! Typed 128-bit identifiers.
//!
//! Every entity id is 16 random bytes formatted as a UUID string
//! (`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`). Ids are generated on the creating
//! device from the injected [`Rng`](crate::Rng), never sequential, never derived
//! from content (see `docs/data-model/domain-model.md`).

use core::fmt;
use core::str::FromStr;
use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::platform::{Rng, RngExt};

/// Error returned when an id cannot be parsed from text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid id: {0}")]
pub struct IdParseError(&'static str);

fn parse_uuid(s: &str) -> Result<[u8; 16], IdParseError> {
    let mut out = [0u8; 16];
    let mut nibbles = 0usize;
    let mut hi = 0u8;
    for b in s.bytes() {
        let v = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            b'-' => continue,
            _ => return Err(IdParseError("unexpected character")),
        };
        if nibbles >= 32 {
            return Err(IdParseError("too many hex digits"));
        }
        if nibbles.is_multiple_of(2) {
            hi = v;
        } else {
            out[nibbles / 2] = (hi << 4) | v;
        }
        nibbles += 1;
    }
    if nibbles != 32 {
        return Err(IdParseError("expected 32 hex digits"));
    }
    Ok(out)
}

fn write_uuid(bytes: &[u8; 16], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 36];
    let mut i = 0;
    for (n, b) in bytes.iter().enumerate() {
        if matches!(n, 4 | 6 | 8 | 10) {
            buf[i] = b'-';
            i += 1;
        }
        buf[i] = HEX[usize::from(b >> 4)];
        buf[i + 1] = HEX[usize::from(b & 0x0f)];
        i += 2;
    }
    // `buf` only ever holds ASCII, so this cannot fail.
    let s = core::str::from_utf8(&buf).map_err(|_| fmt::Error)?;
    f.write_str(s)
}

impl DocId {
    /// The document id of a page's content document **is** the page id (ADR-0016), so
    /// that devices creating the document concurrently converge on one document.
    #[must_use]
    pub const fn for_page(page_id: PageId) -> Self {
        Self(page_id.to_bytes())
    }
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, type = "string"))]
        pub struct $name([u8; 16]);

        impl $name {
            /// Wraps raw bytes unchanged (use for ids read back from storage).
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            /// Builds an id from 16 random bytes, stamping the UUIDv4 version and variant bits.
            #[must_use]
            pub const fn from_random_bytes(mut bytes: [u8; 16]) -> Self {
                bytes[6] = (bytes[6] & 0x0f) | 0x40;
                bytes[8] = (bytes[8] & 0x3f) | 0x80;
                Self(bytes)
            }

            /// Generates a fresh id from the injected CSPRNG.
            #[must_use]
            pub fn generate(rng: &dyn Rng) -> Self {
                Self::from_random_bytes(rng.random_array())
            }

            /// The raw 16 bytes.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }

            /// The raw 16 bytes, by value.
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 16] {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_uuid(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_uuid(s).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s: Cow<'de, str> = Deserialize::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id!(
    /// A workspace: the top-level container a user opens.
    WorkspaceId
);
define_id!(
    /// A space: partition of a workspace, unit of encryption and future sharing (ADR-0004).
    SpaceId
);
define_id!(
    /// A page in a space's page tree.
    PageId
);
define_id!(
    /// A block inside a page document; stable across edits and moves (ADR-0008).
    BlockId
);
define_id!(
    /// A CRDT document (structure, page or account document).
    DocId
);
define_id!(
    /// An attachment (binary file) owned by a space.
    AttachmentId
);
define_id!(
    /// A device of an account; becomes public in the device log.
    DeviceId
);
define_id!(
    /// An account: derived from the device-log genesis entry, opaque to the server.
    AccountId
);
define_id!(
    /// A local installation; names the keystore entry and salts the local DB key.
    /// Stored in plaintext next to the database and never leaves the device.
    InstallId
);
define_id!(
    /// A property of a space's property schema (ADR-0026). The built-in Tags property has
    /// the fixed id [`TAGS_PROPERTY_ID`](crate::properties::TAGS_PROPERTY_ID); every other
    /// property gets a random one.
    PropertyId
);
define_id!(
    /// An option of a select, multi-select or status property (ADR-0026): stable across
    /// renames and recolours, random, never reused.
    OptionId
);
define_id!(
    /// An item of a `checklist` property value (ADR-0026, amendment of 2026-10-07): random, made
    /// by the device that adds the item, stable across edits and reorders.
    ChecklistItemId
);
define_id!(
    /// A sync feed (ADR-0045): the personal feed's id **is** the account id
    /// ([`FeedId::from`] an [`AccountId`]); a shared space's feed has a random id chosen by its
    /// creator. The core records which kind a feed is in `sync_feeds`; the id alone does not say.
    FeedId
);
define_id!(
    /// An invite of a shared space (ADR-0046): `HKDF(s, "invite-id")[..16]` of the invite secret
    /// `s`, so whoever holds `s` can name the invite without the server telling them.
    InviteId
);

define_id!(
    /// A view link (M13 slice 4, ADR-0051): `HKDF(K_link, "link-id")[..16]` of the link key `K_link` that
    /// travels in the URL fragment, so the viewer names the link without telling the server the key.
    LinkId
);

define_id!(
    /// A comment of a page's comment stream (M13 slice 2, ADR-0048): random, made by the device
    /// that posts it, stable across edits. The root comment of a thread names its thread with
    /// the same bytes ([`ThreadId::of`]).
    CommentId
);
define_id!(
    /// A thread of comments on a page (ADR-0048): the id of the thread's root comment, so a
    /// thread's id is known the moment its first comment is posted ([`ThreadId::of`]).
    ThreadId
);
define_id!(
    /// One signed record of a page's comment stream (ADR-0048): random, made by the posting device.
    /// A comment-stream snapshot must contain every record id seen at cursors up to what it covers
    /// (snapshot dominance for records is record-set inclusion).
    CommentRecordId
);

impl ThreadId {
    /// The thread a root comment starts: the same 16 bytes (`thread_id == comment_id` for the root).
    #[must_use]
    pub const fn of(root: CommentId) -> Self {
        Self(root.to_bytes())
    }

    /// The root comment of this thread: the same 16 bytes.
    #[must_use]
    pub const fn root_comment(self) -> CommentId {
        CommentId::from_bytes(self.0)
    }
}

impl CommentId {
    /// The thread this comment would start were it a root: the same 16 bytes.
    #[must_use]
    pub const fn as_thread(self) -> ThreadId {
        ThreadId::of(self)
    }
}

impl FeedId {
    /// The personal feed of an account: the same 16 bytes.
    #[must_use]
    pub const fn from_account(account_id: AccountId) -> Self {
        Self(account_id.to_bytes())
    }

    /// The account whose personal feed this id names (meaningful only when the core's
    /// `sync_feeds` records the feed as personal).
    #[must_use]
    pub const fn as_account(self) -> AccountId {
        AccountId::from_bytes(self.0)
    }

    /// 32 lowercase hex characters, no separators: the form a feed id takes inside key
    /// contexts (`SPACE_KEY_CONTEXT/<feed hex>/<epoch>`, ADR-0044).
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex_lower(&self.0)
    }

    /// The first 8 hex characters: the only form of a feed id that may appear in a log.
    #[must_use]
    pub fn short_hex(&self) -> String {
        hex_lower(&self.0[..4])
    }
}

impl From<AccountId> for FeedId {
    fn from(account_id: AccountId) -> Self {
        Self::from_account(account_id)
    }
}

impl InviteId {
    /// The first 8 hex characters: the only form of an invite id that may appear in a log.
    #[must_use]
    pub fn short_hex(&self) -> String {
        hex_lower(&self.0[..4])
    }
}

impl LinkId {
    /// The first 8 hex characters: the only form of a link id that may appear in a log.
    #[must_use]
    pub fn short_hex(&self) -> String {
        hex_lower(&self.0[..4])
    }
}

/// Lowercase hex of `bytes`.
fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testing::SeededRng;

    #[test]
    fn roundtrips_through_text_and_json() {
        let id = PageId::from_bytes([
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x4d, 0xef, 0x81, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ]);
        let text = id.to_string();
        assert_eq!(text, "12345678-9abc-4def-8123-456789abcdef");
        assert_eq!(text.parse::<PageId>().unwrap(), id);
        assert_eq!(
            "123456789ABC4DEF8123456789ABCDEF"
                .parse::<PageId>()
                .unwrap(),
            id
        );
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"12345678-9abc-4def-8123-456789abcdef\"");
        let back: PageId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        let from_value: PageId = serde_json::from_value(serde_json::json!(text)).unwrap();
        assert_eq!(from_value, id);
        assert_eq!(
            format!("{id:?}"),
            "PageId(12345678-9abc-4def-8123-456789abcdef)"
        );
    }

    #[test]
    fn rejects_malformed_text() {
        assert!("".parse::<PageId>().is_err());
        assert!(
            "12345678-9abc-4def-8123-456789abcde"
                .parse::<PageId>()
                .is_err()
        );
        assert!(
            "12345678-9abc-4def-8123-456789abcdefa"
                .parse::<PageId>()
                .is_err()
        );
        assert!(
            "zz345678-9abc-4def-8123-456789abcdef"
                .parse::<PageId>()
                .is_err()
        );
    }

    #[test]
    fn a_personal_feed_id_is_the_account_id_and_hex_forms_are_fixed() {
        let account = AccountId::from_bytes([
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ]);
        let feed = FeedId::from(account);
        assert_eq!(feed, FeedId::from_account(account));
        assert_eq!(feed.to_bytes(), account.to_bytes());
        assert_eq!(feed.as_account(), account);
        assert_eq!(feed.to_hex(), "123456789abcdef00123456789abcdef");
        assert_eq!(feed.short_hex(), "12345678");
        assert_eq!(
            feed.to_string(),
            "12345678-9abc-def0-0123-456789abcdef",
            "the wire form is the same UUID text as every id"
        );
        let json = serde_json::to_string(&feed).unwrap();
        assert_eq!(json, "\"12345678-9abc-def0-0123-456789abcdef\"");
        assert_eq!(serde_json::from_str::<FeedId>(&json).unwrap(), feed);
        let invite = InviteId::from_bytes([0xab; 16]);
        assert_eq!(invite.short_hex(), "abababab");
        assert_eq!(
            format!("{invite:?}"),
            "InviteId(abababab-abab-abab-abab-abababababab)"
        );
    }

    #[test]
    fn a_link_id_is_a_derived_id_with_the_uuid_text_and_a_short_log_form() {
        // `HKDF(K_link, "link-id")[..16]` is not a random UUID: the bits stay as derived.
        let link = LinkId::from_bytes([0xcd; 16]);
        assert_eq!(link.short_hex(), "cdcdcdcd");
        assert_eq!(link.to_string(), "cdcdcdcd-cdcd-cdcd-cdcd-cdcdcdcdcdcd");
        assert_eq!(
            format!("{link:?}"),
            "LinkId(cdcdcdcd-cdcd-cdcd-cdcd-cdcdcdcdcdcd)"
        );
        let json = serde_json::to_string(&link).unwrap();
        assert_eq!(json, "\"cdcdcdcd-cdcd-cdcd-cdcd-cdcdcdcdcdcd\"");
        assert_eq!(serde_json::from_str::<LinkId>(&json).unwrap(), link);
    }

    #[test]
    fn a_thread_is_named_by_its_root_comment() {
        let root = CommentId::from_bytes([0x5a; 16]);
        let thread = ThreadId::of(root);
        assert_eq!(thread.to_bytes(), root.to_bytes());
        assert_eq!(thread.root_comment(), root);
        assert_eq!(root.as_thread(), thread);
        assert_eq!(
            thread.to_string(),
            root.to_string(),
            "the same UUID text on the wire"
        );
        let record = CommentRecordId::from_bytes([0x5b; 16]);
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            "\"5b5b5b5b-5b5b-5b5b-5b5b-5b5b5b5b5b5b\""
        );
        assert_eq!(
            format!("{thread:?}"),
            "ThreadId(5a5a5a5a-5a5a-5a5a-5a5a-5a5a5a5a5a5a)"
        );
    }

    #[test]
    fn generated_ids_carry_v4_bits_and_differ() {
        let rng = SeededRng::new(7);
        let a = DeviceId::generate(&rng);
        let b = DeviceId::generate(&rng);
        assert_ne!(a, b);
        for id in [a, b] {
            assert_eq!(id.as_bytes()[6] >> 4, 4);
            assert_eq!(id.as_bytes()[8] >> 6, 0b10);
        }
    }
}

/// Content address of one encrypted attachment chunk: BLAKE3-256 of the ciphertext
/// (ADR-0020). Formatted as 64 lowercase hex characters; never derived from plaintext.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, type = "string"))]
pub struct ChunkId([u8; 32]);

impl ChunkId {
    /// Wraps a digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The raw 32 bytes, by value.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Lowercase hex, 64 characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in &self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    /// Parses 64 hex characters of either case.
    pub fn from_hex(text: &str) -> Result<Self, IdParseError> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(IdParseError("a chunk id is 64 hex characters"));
        }
        let mut out = [0u8; 32];
        let (pairs, _) = bytes.as_chunks::<2>();
        for (slot, pair) in out.iter_mut().zip(pairs) {
            let hi = hex_value(pair[0]).ok_or(IdParseError("a chunk id is hex"))?;
            let lo = hex_value(pair[1]).ok_or(IdParseError("a chunk id is hex"))?;
            *slot = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

const fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkId({})", self)
    }
}

impl FromStr for ChunkId {
    type Err = IdParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl Serialize for ChunkId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ChunkId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s: Cow<'de, str> = Deserialize::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod chunk_id_tests {
    use super::*;

    #[test]
    fn chunk_ids_round_trip_through_hex_and_json() {
        let id = ChunkId::from_bytes(core::array::from_fn(|i| i as u8 * 7));
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ChunkId::from_hex(&hex).unwrap(), id);
        assert_eq!(ChunkId::from_hex(&hex.to_uppercase()).unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{hex}\""));
        assert_eq!(serde_json::from_str::<ChunkId>(&json).unwrap(), id);
        assert!(ChunkId::from_hex("abc").is_err());
        assert!(ChunkId::from_hex(&"zz".repeat(32)).is_err());
    }
}
