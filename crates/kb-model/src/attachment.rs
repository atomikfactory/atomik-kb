//! Attachment manifests (ADR-0020, `docs/development/milestone-05.md`, "Model").
//!
//! A manifest describes one attachment: its name, MIME type, plaintext size and hash, and
//! the ids of the encrypted chunks that hold it. Manifests live in the structure document
//! of the attachment's space, in the root map `attachments`, keyed by the attachment id
//! (UUID string, like every other id key). Each manifest is **one plain Loro map value**
//! (not a container): it is written and replaced as a whole, never merged field by field.
//!
//! ```text
//! "<attachment id>": {"v": 1, "name": str, "mime": str, "size": i64, "hash": hex,
//!                     "chunk_size": i64, "chunks": [hex, ..], "created_at": i64,
//!                     "created_by": "<device id>"}
//! ```
//!
//! `hash` (BLAKE3-256 of the plaintext) and the chunk ids are 64 lowercase hex
//! characters; readers refuse any other spelling, so a manifest read back writes back
//! identically. Readers ignore keys they do not know (a later additive field) and refuse
//! any version but 1. The attachment key is not part of the manifest: it is a keyring
//! record. Frozen by `fixtures/doc/structure-v1-attachments.*`.

use std::collections::HashMap;

use kb_types::{AttachmentId, ChunkId, DeviceId};
use loro::LoroValue;

use crate::ModelError;

/// The manifest version this code writes and the only one it reads.
pub const MANIFEST_VERSION: u8 = 1;

/// Keys of a manifest value (ADR-0020).
mod key {
    pub(super) const V: &str = "v";
    pub(super) const NAME: &str = "name";
    pub(super) const MIME: &str = "mime";
    pub(super) const SIZE: &str = "size";
    pub(super) const HASH: &str = "hash";
    pub(super) const CHUNK_SIZE: &str = "chunk_size";
    pub(super) const CHUNKS: &str = "chunks";
    pub(super) const CREATED_AT: &str = "created_at";
    pub(super) const CREATED_BY: &str = "created_by";
}

/// The manifest of one attachment, as stored in its space's structure document.
///
/// The model checks the encoding only; the core validates names, MIME types and limits
/// before it writes a manifest, and [`crate::StructureDoc::check_invariants`] reports
/// manifests whose chunk count does not match their size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentManifest {
    /// Manifest version; always [`MANIFEST_VERSION`].
    pub v: u8,
    /// The attachment (the manifest's key in the `attachments` map).
    pub attachment_id: AttachmentId,
    /// Display name (a filename; never used as a path).
    pub name: String,
    /// MIME type.
    pub mime: String,
    /// Plaintext size in bytes (where the plaintext of the last chunk ends).
    pub size: u64,
    /// BLAKE3-256 of the whole plaintext.
    pub hash: [u8; 32],
    /// Plaintext bytes per chunk (every chunk but the last holds exactly this many).
    pub chunk_size: u32,
    /// Chunk ids in order (BLAKE3-256 of each chunk's ciphertext).
    pub chunks: Vec<ChunkId>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// The device that created the attachment.
    pub created_by: DeviceId,
}

impl AttachmentManifest {
    /// The number of chunks a manifest of `size` bytes cut into `chunk_size` pieces
    /// lists: `floor(size / chunk_size) + 1`. The core always seals a final, padded
    /// chunk (the authenticated end marker), even when `size` is an exact multiple of
    /// `chunk_size`: an empty file has 1 chunk, a file of exactly one chunk size has 2.
    /// `None` when `chunk_size` is 0 (saturates at `u64::MAX`).
    #[must_use]
    pub fn expected_chunk_count(size: u64, chunk_size: u32) -> Option<u64> {
        if chunk_size == 0 {
            return None;
        }
        Some((size / u64::from(chunk_size)).saturating_add(1))
    }

    /// The Loro value stored under the attachment id (see the module documentation).
    ///
    /// Errors with [`ModelError::Shape`] when `v` is not [`MANIFEST_VERSION`] or `size`
    /// does not fit a signed 64-bit integer.
    pub fn to_loro(&self) -> Result<LoroValue, ModelError> {
        if self.v != MANIFEST_VERSION {
            return Err(shape("version is not 1"));
        }
        let size = i64::try_from(self.size).map_err(|_| shape("size is out of range"))?;
        let chunks: Vec<LoroValue> = self
            .chunks
            .iter()
            .map(|chunk| LoroValue::from(chunk.to_hex()))
            .collect();
        let mut map: HashMap<&str, LoroValue> = HashMap::with_capacity(9);
        map.insert(key::V, LoroValue::I64(i64::from(self.v)));
        map.insert(key::NAME, LoroValue::from(self.name.as_str()));
        map.insert(key::MIME, LoroValue::from(self.mime.as_str()));
        map.insert(key::SIZE, LoroValue::I64(size));
        map.insert(key::HASH, LoroValue::from(hex(&self.hash)));
        map.insert(key::CHUNK_SIZE, LoroValue::I64(i64::from(self.chunk_size)));
        map.insert(key::CHUNKS, LoroValue::from(chunks));
        map.insert(key::CREATED_AT, LoroValue::I64(self.created_at));
        map.insert(
            key::CREATED_BY,
            LoroValue::from(self.created_by.to_string()),
        );
        Ok(LoroValue::from(map))
    }

    /// Reads the value stored under `attachment_id`.
    ///
    /// Errors with [`ModelError::Shape`] (a fixed, content-free message) when the value
    /// is not a manifest map, its version is not 1, a field is missing or has the wrong
    /// type, a number is out of range, or a hex string or id is malformed.
    pub fn from_loro(attachment_id: AttachmentId, value: &LoroValue) -> Result<Self, ModelError> {
        if !matches!(value, LoroValue::Map(_)) {
            return Err(shape("is not a map"));
        }
        // The version first: another version may have other fields.
        if int(value, key::V)? != i64::from(MANIFEST_VERSION) {
            return Err(shape("version is not 1"));
        }
        let size = u64::try_from(int(value, key::SIZE)?).map_err(|_| shape("size is negative"))?;
        let chunk_size = u32::try_from(int(value, key::CHUNK_SIZE)?)
            .map_err(|_| shape("chunk_size is out of range"))?;
        let chunks = match value.get_by_key(key::CHUNKS) {
            Some(LoroValue::List(items)) => items
                .iter()
                .map(|item| match item {
                    LoroValue::String(text) => parse_hex(text.as_str()).map(ChunkId::from_bytes),
                    _ => None,
                })
                .collect::<Option<Vec<ChunkId>>>()
                .ok_or_else(|| shape("a chunk id is not 64 lowercase hex characters"))?,
            _ => return Err(shape("chunks is missing or not a list")),
        };
        let hash = parse_hex(&string(value, key::HASH)?)
            .ok_or_else(|| shape("hash is not 64 lowercase hex characters"))?;
        let created_by = string(value, key::CREATED_BY)?
            .parse::<DeviceId>()
            .map_err(|_| shape("created_by is not a device id"))?;
        Ok(Self {
            v: MANIFEST_VERSION,
            attachment_id,
            name: string(value, key::NAME)?,
            mime: string(value, key::MIME)?,
            size,
            hash,
            chunk_size,
            chunks,
            created_at: int(value, key::CREATED_AT)?,
            created_by,
        })
    }

    /// Engine-independent JSON (portable format 1): the manifest fields in camelCase,
    /// keys inserted in sorted order, hex strings as stored. The attachment id is the
    /// key of the enclosing object.
    pub(crate) fn to_portable(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("chunkSize".into(), self.chunk_size.into());
        obj.insert(
            "chunks".into(),
            self.chunks
                .iter()
                .map(|chunk| serde_json::Value::String(chunk.to_hex()))
                .collect::<Vec<_>>()
                .into(),
        );
        obj.insert("createdAt".into(), self.created_at.into());
        obj.insert("createdBy".into(), self.created_by.to_string().into());
        obj.insert("hash".into(), hex(&self.hash).into());
        obj.insert("mime".into(), self.mime.clone().into());
        obj.insert("name".into(), self.name.clone().into());
        obj.insert("size".into(), self.size.into());
        obj.insert("v".into(), self.v.into());
        obj.into()
    }

    /// Consistency problems of a readable manifest (ids and numbers only, never the
    /// name or MIME type).
    pub(crate) fn problems(&self) -> Vec<String> {
        let id = self.attachment_id;
        let mut problems = Vec::new();
        match Self::expected_chunk_count(self.size, self.chunk_size) {
            None => problems.push(format!("attachment {id} has a chunk size of 0")),
            Some(expected) => {
                let listed = u64::try_from(self.chunks.len()).unwrap_or(u64::MAX);
                if listed != expected {
                    problems.push(format!(
                        "attachment {id} lists {listed} chunks but its size needs {expected}"
                    ));
                }
            }
        }
        problems
    }
}

fn shape(what: &str) -> ModelError {
    ModelError::Shape(format!("attachment manifest: {what}"))
}

fn int(value: &LoroValue, key: &str) -> Result<i64, ModelError> {
    match value.get_by_key(key) {
        Some(LoroValue::I64(v)) => Ok(*v),
        _ => Err(shape(&format!("{key} is missing or not an integer"))),
    }
}

fn string(value: &LoroValue, key: &str) -> Result<String, ModelError> {
    match value.get_by_key(key) {
        Some(LoroValue::String(s)) => Ok(s.to_string()),
        _ => Err(shape(&format!("{key} is missing or not a string"))),
    }
}

/// Lowercase hex of 32 bytes.
fn hex(bytes: &[u8; 32]) -> String {
    // A chunk id has the same text form as any other 32-byte digest.
    ChunkId::from_bytes(*bytes).to_hex()
}

/// 32 bytes from exactly 64 lowercase hex characters.
fn parse_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    ChunkId::from_hex(text).ok().map(ChunkId::to_bytes)
}
