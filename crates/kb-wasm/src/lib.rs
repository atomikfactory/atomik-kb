//! The web viewer's WebAssembly module (M13 slice 4, ADR-0051, `docs/security/sharing.md` section 10,
//! `docs/development/milestone-13.md` 4.2 and its contract notes): what `apps/viewer` needs from the
//! Rust core and nothing else. It opens a view link's publication, derives the link id and decrypts
//! the chunks of the attachments the page uses, over `kb-crypto` and `kb-sync` only: **no SQLite, no
//! storage crate, no clang**, so it builds for `wasm32-unknown-unknown` with the clang-free check
//! (`cargo check --target wasm32-unknown-unknown -p kb-wasm`).
//!
//! # The surface JavaScript sees
//!
//! ```text
//! linkId(fragment: string): string                      the UUID text of the link id the key derives
//! openPublication(fragment, bytes: Uint8Array): OpenedPublication
//! OpenedPublication
//!   .pageId: string     .title: string     .icon?: string     .cover?: string     .coverY: number
//!   .snapshot(): Uint8Array             a `kb-doc` snapshot blob (`KBD1` header, then the engine's shallow snapshot)
//!   .attachmentsJson(): string          [{"attachmentId","name","mime","size","hash","chunkSize","chunks":[hex,..]}]
//!   .mentionsJson(): string             [{"accountId","name"}]
//!   .decryptChunk(attachmentId: string, index: number, isFinal: boolean, sealed: Uint8Array): Uint8Array
//! ```
//!
//! `fragment` is what follows the `#` of the link's URL (`location.hash`; a leading `#` is fine). The
//! attachment keys stay inside the module: `decryptChunk` looks the key of `attachmentId` up in the
//! publication, so the page's script never holds one. A sealed chunk is the bytes
//! `GET /v2/links/{id}/chunks/{hash}` answers; `index` is its position in the attachment's `chunks`
//! list and `isFinal` is whether it is the last one (the chunk AAD binds both, so a chunk offered at
//! another position fails). `decryptChunk` answers the chunk's **plaintext**: the final chunk of an
//! attachment is padded when it is sealed (a `u32` length, the bytes, random padding), and the module
//! removes that padding, so the chunks' answers concatenate to the attachment's bytes. It also checks
//! that the sealed bytes hash to the chunk id the publication lists (so a chunk the server swapped for
//! another one is `authentication`, whatever its content).
//!
//! **Errors** reach JavaScript as an `Error` whose message is `"<code>: <text>"`, with the code one of
//! `bad_key` (the fragment is not a link key), `authentication` (the bytes do not open under the key:
//! the wrong key, or a publication that was altered or belongs to another link), `malformed` (the
//! bytes are not a publication, or a chunk is not a chunk) and `unknown_attachment`. The text never
//! repeats the key or the bytes. See [`ViewerError`].
//!
//! # Layout
//!
//! The logic is plain Rust ([`link_id_of`], [`open`], [`OpenedPublication::decrypt`]), testable on any
//! target (the vectors of `kb-crypto` and `kb-sync` run natively; `wasm-bindgen-test` runs the exports
//! in a browser). The `#[wasm_bindgen]` functions are one-line wrappers that turn a [`ViewerError`]
//! into a `JsError` (which can only be made on `wasm32`).
//!
//! `unsafe` is not used here; the macro-generated glue of `wasm-bindgen` is the only code that may need
//! it, and the workspace lint decides (see the crate's `Cargo.toml`).

use kb_crypto::open_chunk_at;
use kb_sync::SyncError;
use kb_sync::v2::{LinkKey, Publication, open_publication as open_sealed};
use kb_types::attachments::ATTACHMENT_AAD_DOMAIN;
use kb_types::{ChunkId, LinkId};
use serde_json::json;
use wasm_bindgen::prelude::*;

/// Why an export failed. The text never holds a key or bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewerError {
    /// The fragment is not a link key (43 characters of base64url).
    #[error("bad_key: the link's key is not valid")]
    BadKey,
    /// The bytes do not open under the key: the wrong key, a publication altered on the way or made
    /// for another link, or a chunk that is not the chunk offered.
    #[error("authentication: the content does not match the link")]
    Authentication,
    /// The bytes are not a publication or a chunk of this version.
    #[error("malformed: {0}")]
    Malformed(String),
    /// The publication has no attachment with that id.
    #[error("unknown_attachment: the page has no such attachment")]
    UnknownAttachment,
}

/// The link id a fragment's key derives (`HKDF(K_link, "link-id")[..16]`): the id `GET /v2/links/{id}`
/// takes.
pub fn link_id_of(fragment: &str) -> Result<LinkId, ViewerError> {
    let key = LinkKey::from_fragment(fragment).map_err(|_| ViewerError::BadKey)?;
    Ok(key.link_id())
}

/// Opens a publication with the key of `fragment` (see [`kb_sync::v2::open_publication`]): the
/// fragment must be a key ([`ViewerError::BadKey`]), the bytes must open under it
/// ([`ViewerError::Authentication`]) and decode ([`ViewerError::Malformed`]).
pub fn open(fragment: &str, bytes: &[u8]) -> Result<OpenedPublication, ViewerError> {
    let key = LinkKey::from_fragment(fragment).map_err(|_| ViewerError::BadKey)?;
    let publication = open_sealed(&key, bytes).map_err(publication_error)?;
    Ok(OpenedPublication::new(publication))
}

/// What a failure to open a publication is to the page: a tag or a header that does not match is the
/// wrong key (or a publication of another link); everything else is bytes that are not a publication.
/// The text keeps the codec's own words (they name a field, never the bytes).
fn publication_error(error: SyncError) -> ViewerError {
    match error {
        SyncError::Authentication => ViewerError::Authentication,
        SyncError::Encoding(text) | SyncError::Protocol(text) => ViewerError::Malformed(text),
        other => ViewerError::Malformed(other.to_string()),
    }
}

/// An opened publication: what the page shows and the keys of its attachments, held inside the
/// module. Exported to JavaScript as `OpenedPublication`.
#[wasm_bindgen]
pub struct OpenedPublication {
    inner: Publication,
}

impl OpenedPublication {
    /// Wraps a decoded publication (the result of [`kb_sync::v2::open_publication`]).
    #[must_use]
    pub const fn new(inner: Publication) -> Self {
        Self { inner }
    }

    /// The attachments as the JSON of `attachmentsJson()`.
    #[must_use]
    pub fn attachments_value(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.inner
                .attachments
                .iter()
                .map(|attachment| {
                    json!({
                        "attachmentId": attachment.attachment_id.to_string(),
                        "name": attachment.name,
                        "mime": attachment.mime,
                        "size": attachment.size,
                        "hash": hex(&attachment.hash),
                        "chunkSize": attachment.chunk_size,
                        "chunks": attachment.chunks.iter().map(|chunk| chunk.to_hex()).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    }

    /// The mentioned people as the JSON of `mentionsJson()`.
    #[must_use]
    pub fn mentions_value(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.inner
                .mentions
                .iter()
                .map(|person| {
                    json!({ "accountId": person.account_id.to_string(), "name": person.name })
                })
                .collect(),
        )
    }

    /// Decrypts one sealed chunk of an attachment of the page with the attachment's key (the chunk AAD
    /// is `ATTACHMENT_AAD_DOMAIN` plus the attachment id, the position and whether it is final: the
    /// function of `kb_crypto::open_chunk_at`) and answers its plaintext, without the padding of a
    /// final chunk.
    ///
    /// Errors: [`ViewerError::UnknownAttachment`] for an id the page does not use;
    /// [`ViewerError::Malformed`] for an `index` past the attachment's last chunk and for a final chunk
    /// whose padding does not parse; [`ViewerError::Authentication`] for sealed bytes that are not the
    /// chunk the publication lists at `index` (their hash differs), that do not open under the
    /// attachment's key at that position, or that were sealed with the other `is_final`.
    pub fn decrypt(
        &self,
        attachment_id: &str,
        index: u32,
        is_final: bool,
        sealed: &[u8],
    ) -> Result<Vec<u8>, ViewerError> {
        let attachment = self
            .inner
            .attachments
            .iter()
            .find(|attachment| attachment.attachment_id.to_string() == attachment_id)
            .ok_or(ViewerError::UnknownAttachment)?;
        let listed = usize::try_from(index)
            .ok()
            .and_then(|position| attachment.chunks.get(position))
            .ok_or_else(|| {
                ViewerError::Malformed("the chunk is past the attachment's end".into())
            })?;
        if ChunkId::from_bytes(kb_crypto::hash(sealed)) != *listed {
            return Err(ViewerError::Authentication);
        }
        let mut prefix = Vec::with_capacity(ATTACHMENT_AAD_DOMAIN.len() + 16);
        prefix.extend_from_slice(ATTACHMENT_AAD_DOMAIN);
        prefix.extend_from_slice(attachment.attachment_id.as_bytes());
        let plain = open_chunk_at(&attachment.key, &prefix, u64::from(index), sealed, is_final)
            .map_err(|_| ViewerError::Authentication)?;
        if is_final {
            unpad_final(&plain).map(<[u8]>::to_vec).ok_or_else(|| {
                ViewerError::Malformed("the final chunk's padding is damaged".into())
            })
        } else {
            Ok(plain.to_vec())
        }
    }
}

#[wasm_bindgen]
impl OpenedPublication {
    /// The page's id (UUID text).
    #[wasm_bindgen(getter = pageId)]
    #[must_use]
    pub fn page_id(&self) -> String {
        self.inner.page_id.to_string()
    }

    /// The page's title.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn title(&self) -> String {
        self.inner.title.clone()
    }

    /// The page's icon, when it has one.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn icon(&self) -> Option<String> {
        self.inner.icon.clone()
    }

    /// The page's cover value (`attachment:<id>`, `color:<name>` or `gradient:<name>`), when it has one.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn cover(&self) -> Option<String> {
        self.inner.cover.clone()
    }

    /// The cover's vertical position in percent (0 to 100).
    #[wasm_bindgen(getter = coverY)]
    #[must_use]
    pub fn cover_y(&self) -> u8 {
        self.inner.cover_y
    }

    /// The page document: a `kb-doc` snapshot blob without history (the `KBD1` header, then the
    /// engine's shallow snapshot, which `loro-crdt` imports).
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        self.inner.snapshot.clone()
    }

    /// The attachments the page uses, as JSON text (see the module documentation).
    #[wasm_bindgen(js_name = attachmentsJson)]
    #[must_use]
    pub fn attachments_json(&self) -> String {
        self.attachments_value().to_string()
    }

    /// The people the page mentions, as JSON text (see the module documentation).
    #[wasm_bindgen(js_name = mentionsJson)]
    #[must_use]
    pub fn mentions_json(&self) -> String {
        self.mentions_value().to_string()
    }

    /// Decrypts one sealed chunk of an attachment (see [`OpenedPublication::decrypt`]).
    #[wasm_bindgen(js_name = decryptChunk)]
    pub fn decrypt_chunk(
        &self,
        attachment_id: &str,
        index: u32,
        is_final: bool,
        sealed: &[u8],
    ) -> Result<Vec<u8>, JsError> {
        self.decrypt(attachment_id, index, is_final, sealed)
            .map_err(js_error)
    }
}

/// `linkId(fragment)`: see [`link_id_of`].
#[wasm_bindgen(js_name = linkId)]
pub fn link_id(fragment: &str) -> Result<String, JsError> {
    link_id_of(fragment)
        .map(|id| id.to_string())
        .map_err(js_error)
}

/// `openPublication(fragment, bytes)`: see [`open`].
#[wasm_bindgen(js_name = openPublication)]
pub fn open_publication(fragment: &str, bytes: &[u8]) -> Result<OpenedPublication, JsError> {
    open(fragment, bytes).map_err(js_error)
}

/// The error JavaScript gets: the message is the error's `"<code>: <text>"`.
fn js_error(error: ViewerError) -> JsError {
    JsError::new(&error.to_string())
}

/// Bytes of the length prefix of a final chunk (`kb-core`'s `pad_final`: `u32` LE length, the bytes, then
/// random padding).
const FINAL_PREFIX: usize = 4;

/// The bytes of a padded final chunk: inverse of `kb-core`'s `pad_final`.
fn unpad_final(padded: &[u8]) -> Option<&[u8]> {
    let (prefix, rest) = padded.split_first_chunk::<FINAL_PREFIX>()?;
    let length = usize::try_from(u32::from_le_bytes(*prefix)).ok()?;
    rest.get(..length)
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
