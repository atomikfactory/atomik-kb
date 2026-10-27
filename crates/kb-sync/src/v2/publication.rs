//! View-link publications (M13 slice 4, ADR-0051, `docs/security/sharing.md` section 10,
//! `docs/development/milestone-13.md` section 4.1 and its contract notes): the sealed object a view
//! link serves, the `PUT` that uploads it, and what the viewer's `kb-wasm` opens. This module is
//! everything of the link protocol that is not a trait method of [`RemoteStore`](super::RemoteStore).
//!
//! # The publication (envelope kind `Publication`, unsigned)
//!
//! ```text
//! envelope  kind 11, header = u8 2 | link id[16]                                          (17 bytes)
//!           key    = HKDF(K_link, "publication")      (`LinkKey::publication_key`)
//!           body   = the plaintext below, NOT bucket-padded (the server learns the publication's size
//!                    anyway, ADR-0051) and NOT signed: whoever holds `K_link` may have made it, and
//!                    the viewer trusts the server that serves it for that one page
//! ```
//!
//! The header names the link, so the server can check that a `PUT` for `{id}` carries a publication
//! for `{id}` ([`peek_publication`]), and the AEAD binds it: a publication moved to another link id
//! does not open, and [`open_publication`] refuses one whose header differs from the id the key
//! derives.
//!
//! # The plaintext (all integers little-endian; `str` and `bytes` are a `u32` length and the data)
//!
//! ```text
//! u8     version = 1                              (PUBLICATION_VERSION; another value is refused)
//! [16]   page id
//! str    title                  UTF-8, at most PUBLICATION_TITLE_MAX_BYTES
//! str    icon                   UTF-8, at most PUBLICATION_SHORT_MAX_BYTES; empty = none
//! str    cover                  the page's cover value (`attachment:<id>`, `color:<name>`,
//!                               `gradient:<name>`), at most PUBLICATION_SHORT_MAX_BYTES; empty = none
//! u8     cover_y                0 to 100, the cover's vertical position in percent
//! bytes  snapshot               a `kb-doc` snapshot blob without history (`Doc::export_shallow_snapshot`:
//!                               the 10-byte `KBD1` header, then the engine's shallow snapshot)
//! u32    attachment count       at most LINK_MAX_CHUNKS (each attachment has at least one chunk)
//!        then, per attachment:
//!          [16] attachment id
//!          [32] AK              the attachment key (a view link hands it to whoever holds the link)
//!          str  name            at most PUBLICATION_SHORT_MAX_BYTES (attachment names are at most 255 bytes)
//!          str  mime            at most PUBLICATION_SHORT_MAX_BYTES
//!          u64  size            plaintext bytes
//!          [32] hash            BLAKE3 of the plaintext
//!          u32  chunk_size      plaintext bytes per chunk
//!          u32  chunk count     at least 1 (the final padded chunk always exists)
//!          [32] x chunk count   chunk ids, in order (BLAKE3 of each sealed chunk)
//! u32    mention count          at most PUBLICATION_MAX_MENTIONS
//!        then, per mention:
//!          [16] account id      the person a `personMention` of the page names
//!          str  name            the name the space knew them by when the publication was made,
//!                               at most PUBLICATION_SHORT_MAX_BYTES
//! ```
//!
//! No comments, no history, no other page, no property values, no member list. The attachments are
//! the ones the page's blocks, icon and cover reference. The sealed chunks themselves are not inside:
//! they stay in the feed's chunk namespace, the `PUT` declares their hashes ([`LinkPut::chunks`]) and
//! the server serves a declared hash at `GET /v2/links/{id}/chunks/{hash}`; a viewer decrypts a chunk
//! with the attachment's key, exactly as members do (`kb_crypto::open_chunk_at`, AAD prefix
//! `ATTACHMENT_AAD_DOMAIN` plus the attachment id). Decoders refuse truncation, trailing bytes,
//! counts the input cannot hold, text that is not UTF-8, a `cover_y` over 100, a manifest whose chunk
//! count is not `size / chunk_size + 1` and any other version, and never pre-allocate from a claimed
//! count. Encoders refuse what a decoder would.
//!
//! # The upload (`PUT /v2/links/{id}`) and the answers of the link calls
//!
//! ```text
//! body     u8 2 | feed[16] | u8 flags | i64 expires_at | u32 chunk count | [32] x chunk count | publication
//!          flags: bit 0 = the link expires at `expires_at` (otherwise `expires_at` is 0); the rest are 0
//! 200      {"pubSeq": N}   the server's counter for the link: 1 at the first upload, +1 per upload
//! GET      200 the publication bytes, `ETag: "N"`; 304 when `If-None-Match` is `"N"`; 404 otherwise
//! ```
//!
//! Everything in this module is pure and wasm32-capable (the viewer's `kb-wasm` uses
//! [`open_publication`] and the link helpers).
//!
//! The golden `fixtures/sync/publication-v2.bin` (made by `examples/make_sync_vectors_v2.rs`) is a
//! sealed publication with two attachments and two mentions; `tests/goldens_v2.rs` opens it.

use kb_crypto::{EnvelopeKind, Secret32};
pub use kb_keys::link::{LINK_KEY_FRAGMENT_LEN, LinkKey};
use kb_types::sharing::LINK_MAX_CHUNKS;
use kb_types::sync::PROTOCOL_VERSION;
use kb_types::{AccountId, AttachmentId, ChunkId, FeedId, LinkId, PageId, Rng};
use zeroize::Zeroizing;

use super::{read_feed, read_version};
use crate::codec::{Reader, put_blob, put_count};
use crate::{SyncError, crypto_error, decode_envelope};

/// The version byte of the publication plaintext.
pub const PUBLICATION_VERSION: u8 = 1;
/// Bytes of a publication envelope's header: `u8 2 | link id`.
pub const PUBLICATION_HEADER_LEN: usize = 1 + 16;
/// The most bytes a page title takes in a publication.
pub const PUBLICATION_TITLE_MAX_BYTES: usize = 4096;
/// The most bytes an icon, a cover, a name or a MIME type takes in a publication.
pub const PUBLICATION_SHORT_MAX_BYTES: usize = 1024;
/// The most people a publication names (the mentions of the page).
pub const PUBLICATION_MAX_MENTIONS: usize = 1024;
/// The largest vertical cover position, in percent.
pub const PUBLICATION_COVER_Y_MAX: u8 = 100;
/// Bytes of the upload body before the declared chunk hashes: `2 | feed | flags | expires_at | count`.
pub const LINK_PUT_PREFIX_LEN: usize = 1 + 16 + 1 + 8 + 4;
/// The flag of an upload that carries an expiry.
const LINK_PUT_FLAG_EXPIRES: u8 = 1;

// ----- the publication ------------------------------------------------------------------------

/// What a publication says about one attachment the page uses: its key and its manifest, so a viewer
/// can fetch and decrypt its chunks without any other access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedAttachment {
    /// The attachment.
    pub attachment_id: AttachmentId,
    /// The attachment key `AK`. A view link hands it to whoever holds the link.
    pub key: Secret32,
    /// Display name (a filename).
    pub name: String,
    /// MIME type.
    pub mime: String,
    /// Plaintext size in bytes.
    pub size: u64,
    /// BLAKE3 of the plaintext.
    pub hash: [u8; 32],
    /// Plaintext bytes per chunk.
    pub chunk_size: u32,
    /// Chunk ids in order (BLAKE3 of each sealed chunk): the hashes the upload declares.
    pub chunks: Vec<ChunkId>,
}

/// A person a mention in the page names, with the name the space knew them by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPerson {
    /// The account.
    pub account_id: AccountId,
    /// Their name.
    pub name: String,
}

/// The plaintext of a publication (see the [module documentation](self) for the bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// The page.
    pub page_id: PageId,
    /// The page's title.
    pub title: String,
    /// The page's icon; `None` for none.
    pub icon: Option<String>,
    /// The page's cover value; `None` for none.
    pub cover: Option<String>,
    /// The cover's vertical position in percent (0 to [`PUBLICATION_COVER_Y_MAX`]).
    pub cover_y: u8,
    /// A `kb-doc` snapshot blob without history (`Doc::export_shallow_snapshot`).
    pub snapshot: Vec<u8>,
    /// The attachments the page, its icon and its cover reference.
    pub attachments: Vec<PublishedAttachment>,
    /// The people the page's mentions name.
    pub mentions: Vec<PublishedPerson>,
}

impl Publication {
    /// Every chunk hash the publication's attachments list, in attachment order: what the upload
    /// declares (`LinkPut::chunks`). More than [`LINK_MAX_CHUNKS`] is for the caller to refuse.
    #[must_use]
    pub fn chunk_ids(&self) -> Vec<ChunkId> {
        self.attachments
            .iter()
            .flat_map(|attachment| attachment.chunks.iter().copied())
            .collect()
    }
}

/// The plaintext header of a publication envelope: what a server reads and checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationHeader {
    /// The link the publication is for (`HKDF(K_link, "link-id")[..16]`).
    pub link_id: LinkId,
}

/// The header bytes of a publication ([`PUBLICATION_HEADER_LEN`]); the envelope header of
/// [`seal_publication`].
#[must_use]
pub fn encode_publication_header(header: &PublicationHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(PUBLICATION_HEADER_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(header.link_id.as_bytes());
    out
}

fn decode_publication_header(bytes: &[u8]) -> Result<PublicationHeader, SyncError> {
    let mut r = Reader::new(bytes, "publication header");
    read_version(&mut r)?;
    let header = PublicationHeader {
        link_id: LinkId::from_bytes(r.array("link id")?),
    };
    r.finish()?;
    Ok(header)
}

/// Reads the header of a publication envelope without a key: what a server checks (the link id of
/// the path against the publication's). Malformed bytes, another envelope kind, a signature (a
/// publication is unsigned) and a v1 header are errors.
pub fn peek_publication(bytes: &[u8]) -> Result<PublicationHeader, SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Publication)?;
    if envelope.signature.is_some() {
        return Err(SyncError::Encoding(
            "a publication carries no signature".to_owned(),
        ));
    }
    decode_publication_header(&envelope.header)
}

/// Appends `u32 LE len || bytes`, refusing text over `max` bytes (what the decoder refuses).
fn put_text(out: &mut Vec<u8>, text: &str, max: usize, field: &str) -> Result<(), SyncError> {
    if text.len() > max {
        return Err(SyncError::Protocol(format!(
            "a publication's {field} takes at most {max} bytes"
        )));
    }
    put_blob(out, text.as_bytes())
        .ok_or_else(|| SyncError::Protocol(format!("a publication's {field} is too long")))
}

/// Reads a `str` field: a `u32` length, at most `max` bytes of UTF-8.
fn read_text(r: &mut Reader<'_>, max: usize, field: &str) -> Result<String, SyncError> {
    let bytes = r.blob(field)?;
    if bytes.len() > max {
        return Err(SyncError::Encoding(format!(
            "publication: {field} of {} bytes exceeds {max}",
            bytes.len()
        )));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|_| SyncError::Encoding(format!("publication: {field} is not UTF-8")))
}

/// The chunk count a manifest of `size` plaintext bytes in chunks of `chunk_size` must list: the
/// data chunks plus the final padded chunk, which always exists (`size / chunk_size + 1`).
fn expected_chunk_count(size: u64, chunk_size: u32) -> Option<u64> {
    (size / u64::from(chunk_size.max(1))).checked_add(1)
}

/// The plaintext bytes of a publication (the layout of the [module documentation](self)); they hold
/// attachment keys, so they stay zeroised. Refuses what a decoder would: a title, icon, cover, name
/// or MIME type over its limit, a `cover_y` over 100, more than [`LINK_MAX_CHUNKS`] attachments,
/// more than [`PUBLICATION_MAX_MENTIONS`] mentions, a manifest whose chunk count is not
/// `size / chunk_size + 1`, a zero `chunk_size`. An icon or cover that is `Some("")` is written as
/// none (the wire has no difference), so it reads back as `None`.
pub fn encode_publication(publication: &Publication) -> Result<Zeroizing<Vec<u8>>, SyncError> {
    if publication.cover_y > PUBLICATION_COVER_Y_MAX {
        return Err(SyncError::Protocol(format!(
            "a publication's cover_y is at most {PUBLICATION_COVER_Y_MAX}"
        )));
    }
    if publication.attachments.len() > LINK_MAX_CHUNKS {
        return Err(SyncError::Protocol(format!(
            "a publication names at most {LINK_MAX_CHUNKS} attachments"
        )));
    }
    if publication.mentions.len() > PUBLICATION_MAX_MENTIONS {
        return Err(SyncError::Protocol(format!(
            "a publication names at most {PUBLICATION_MAX_MENTIONS} people"
        )));
    }
    let manifests: usize = publication
        .attachments
        .iter()
        .map(|attachment| 16 + 32 + 8 + 4 + 8 + 32 + 8 + attachment.chunks.len() * 32)
        .sum();
    let mut out = Zeroizing::new(Vec::with_capacity(
        1 + 16 + 4 + publication.title.len() + 16 + 1 + 4 + publication.snapshot.len() + manifests,
    ));
    out.push(PUBLICATION_VERSION);
    out.extend_from_slice(publication.page_id.as_bytes());
    put_text(
        &mut out,
        &publication.title,
        PUBLICATION_TITLE_MAX_BYTES,
        "title",
    )?;
    put_text(
        &mut out,
        publication.icon.as_deref().unwrap_or(""),
        PUBLICATION_SHORT_MAX_BYTES,
        "icon",
    )?;
    put_text(
        &mut out,
        publication.cover.as_deref().unwrap_or(""),
        PUBLICATION_SHORT_MAX_BYTES,
        "cover",
    )?;
    out.push(publication.cover_y);
    put_blob(&mut out, &publication.snapshot)
        .ok_or_else(|| SyncError::Protocol("a publication's snapshot is too long".to_owned()))?;
    put_count(&mut out, publication.attachments.len())
        .ok_or_else(|| SyncError::Protocol("too many attachments".to_owned()))?;
    for attachment in &publication.attachments {
        if attachment.chunk_size == 0 {
            return Err(SyncError::Protocol(
                "an attachment's chunk size is not zero".to_owned(),
            ));
        }
        let expected = expected_chunk_count(attachment.size, attachment.chunk_size);
        if expected != u64::try_from(attachment.chunks.len()).ok() {
            return Err(SyncError::Protocol(
                "an attachment lists the wrong number of chunks for its size".to_owned(),
            ));
        }
        out.extend_from_slice(attachment.attachment_id.as_bytes());
        out.extend_from_slice(attachment.key.expose());
        put_text(
            &mut out,
            &attachment.name,
            PUBLICATION_SHORT_MAX_BYTES,
            "attachment name",
        )?;
        put_text(
            &mut out,
            &attachment.mime,
            PUBLICATION_SHORT_MAX_BYTES,
            "attachment type",
        )?;
        out.extend_from_slice(&attachment.size.to_le_bytes());
        out.extend_from_slice(&attachment.hash);
        out.extend_from_slice(&attachment.chunk_size.to_le_bytes());
        put_count(&mut out, attachment.chunks.len())
            .ok_or_else(|| SyncError::Protocol("too many chunks".to_owned()))?;
        for chunk in &attachment.chunks {
            out.extend_from_slice(chunk.as_bytes());
        }
    }
    put_count(&mut out, publication.mentions.len())
        .ok_or_else(|| SyncError::Protocol("too many mentions".to_owned()))?;
    for person in &publication.mentions {
        out.extend_from_slice(person.account_id.as_bytes());
        put_text(
            &mut out,
            &person.name,
            PUBLICATION_SHORT_MAX_BYTES,
            "mentioned name",
        )?;
    }
    Ok(out)
}

/// Parses [`encode_publication`] output strictly (see the [module documentation](self) for what is
/// refused). Errors are [`SyncError::Encoding`] naming the field, never the bytes.
pub fn decode_publication(bytes: &[u8]) -> Result<Publication, SyncError> {
    let mut r = Reader::new(bytes, "publication");
    let version = r.u8("version")?;
    if version != PUBLICATION_VERSION {
        return Err(SyncError::Encoding(format!(
            "publication: unsupported version {version}"
        )));
    }
    let page_id = PageId::from_bytes(r.array("page id")?);
    let title = read_text(&mut r, PUBLICATION_TITLE_MAX_BYTES, "title")?;
    let icon = read_text(&mut r, PUBLICATION_SHORT_MAX_BYTES, "icon")?;
    let cover = read_text(&mut r, PUBLICATION_SHORT_MAX_BYTES, "cover")?;
    let cover_y = r.u8("cover position")?;
    if cover_y > PUBLICATION_COVER_Y_MAX {
        return Err(SyncError::Encoding(
            "publication: cover position is over 100".to_owned(),
        ));
    }
    let snapshot = r.blob("snapshot")?.to_vec();

    let count = r.u32("attachment count")?;
    if usize::try_from(count).is_ok_and(|count| count > LINK_MAX_CHUNKS) {
        return Err(SyncError::Encoding(format!(
            "publication: more than {LINK_MAX_CHUNKS} attachments"
        )));
    }
    // An attachment takes at least id, key, two empty strings, size, hash, chunk size, count and
    // one chunk id.
    let count = r.fit_count(
        count,
        "attachment count",
        16 + 32 + 4 + 4 + 8 + 32 + 4 + 4 + 32,
    )?;
    let mut attachments = Vec::with_capacity(count);
    for _ in 0..count {
        let attachment_id = AttachmentId::from_bytes(r.array("attachment id")?);
        let key = Secret32::from_bytes(r.array("attachment key")?);
        let name = read_text(&mut r, PUBLICATION_SHORT_MAX_BYTES, "attachment name")?;
        let mime = read_text(&mut r, PUBLICATION_SHORT_MAX_BYTES, "attachment type")?;
        let size = r.u64("attachment size")?;
        let hash = r.array("attachment hash")?;
        let chunk_size = r.u32("chunk size")?;
        if chunk_size == 0 {
            return Err(SyncError::Encoding(
                "publication: an attachment's chunk size is zero".to_owned(),
            ));
        }
        let chunk_count = r.count("chunk count", 32)?;
        if expected_chunk_count(size, chunk_size) != u64::try_from(chunk_count).ok() {
            return Err(SyncError::Encoding(
                "publication: an attachment lists the wrong number of chunks".to_owned(),
            ));
        }
        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            chunks.push(ChunkId::from_bytes(r.array("chunk id")?));
        }
        attachments.push(PublishedAttachment {
            attachment_id,
            key,
            name,
            mime,
            size,
            hash,
            chunk_size,
            chunks,
        });
    }

    let count = r.u32("mention count")?;
    if usize::try_from(count).is_ok_and(|count| count > PUBLICATION_MAX_MENTIONS) {
        return Err(SyncError::Encoding(format!(
            "publication: more than {PUBLICATION_MAX_MENTIONS} mentions"
        )));
    }
    let count = r.fit_count(count, "mention count", 16 + 4)?;
    let mut mentions = Vec::with_capacity(count);
    for _ in 0..count {
        let account_id = AccountId::from_bytes(r.array("mentioned account id")?);
        let name = read_text(&mut r, PUBLICATION_SHORT_MAX_BYTES, "mentioned name")?;
        mentions.push(PublishedPerson { account_id, name });
    }
    r.finish()?;
    Ok(Publication {
        page_id,
        title,
        icon: (!icon.is_empty()).then_some(icon),
        cover: (!cover.is_empty()).then_some(cover),
        cover_y,
        snapshot,
        attachments,
        mentions,
    })
}

/// Seals a publication under `link_key`: the envelope of kind `Publication` with the header of
/// [`encode_publication_header`] (the link id the key derives) and the plaintext of
/// [`encode_publication`], encrypted under `link_key.publication_key()`, unsigned. The nonce comes
/// from `rng`.
pub fn seal_publication(
    link_key: &LinkKey,
    publication: &Publication,
    rng: &dyn Rng,
) -> Result<Vec<u8>, SyncError> {
    let plaintext = encode_publication(publication)?;
    let header = encode_publication_header(&PublicationHeader {
        link_id: link_key.link_id(),
    });
    let sealed = kb_crypto::seal_envelope(
        EnvelopeKind::Publication,
        &header,
        &link_key.publication_key(),
        &plaintext,
        None,
        rng,
    );
    if sealed.is_empty() {
        return Err(SyncError::Protocol(
            "the publication is too large".to_owned(),
        ));
    }
    Ok(sealed)
}

/// Opens a publication: decodes the envelope, refuses one whose header names another link than
/// `link_key` derives (or a signed one), decrypts it under `link_key.publication_key()` and decodes
/// the plaintext. A wrong key or tampered bytes are [`SyncError::Authentication`]; malformed bytes
/// are [`SyncError::Encoding`]. This is what `kb-wasm`'s `open_publication` calls.
pub fn open_publication(link_key: &LinkKey, bytes: &[u8]) -> Result<Publication, SyncError> {
    let envelope = decode_envelope(bytes, EnvelopeKind::Publication)?;
    if envelope.signature.is_some() {
        return Err(SyncError::Encoding(
            "a publication carries no signature".to_owned(),
        ));
    }
    let header = decode_publication_header(&envelope.header)?;
    // A publication moved to another link names that link: the key does not belong to it.
    if header.link_id != link_key.link_id() {
        return Err(SyncError::Authentication);
    }
    let plaintext = kb_crypto::aead_open(
        &link_key.publication_key(),
        &envelope.nonce,
        &envelope.aad(),
        &envelope.ciphertext,
    )
    .map_err(crypto_error)?;
    decode_publication(&plaintext)
}

// ----- the upload -----------------------------------------------------------------------------

/// One `PUT /v2/links/{id}` body: the feed the link belongs to, an expiry, the chunk hashes the
/// server may serve, and the sealed publication. The link id is the path's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPut {
    /// The feed the link's page lives in (the account's personal feed for a personal space).
    pub feed: FeedId,
    /// When the link stops working, Unix ms; `None` never. The first upload of a link fixes it; later
    /// uploads of the same link repeat it and the server keeps the stored one.
    pub expires_at: Option<i64>,
    /// The chunk hashes of the publication's attachments (`Publication::chunk_ids`) the server may
    /// serve to anyone holding the link id: at most [`LINK_MAX_CHUNKS`], and only chunks the feed
    /// holds are served.
    pub chunks: Vec<ChunkId>,
    /// The sealed publication (`seal_publication` output).
    pub publication: Vec<u8>,
}

/// The body of a `PUT /v2/links/{id}` (see the [module documentation](self)). More than
/// [`LINK_MAX_CHUNKS`] declared chunks or an empty publication is [`SyncError::Protocol`].
pub fn encode_link_put(put: &LinkPut) -> Result<Vec<u8>, SyncError> {
    if put.chunks.len() > LINK_MAX_CHUNKS {
        return Err(SyncError::Protocol(format!(
            "a link declares at most {LINK_MAX_CHUNKS} chunks"
        )));
    }
    if put.publication.is_empty() {
        return Err(SyncError::Protocol(
            "a link upload carries a publication".to_owned(),
        ));
    }
    let count = u32::try_from(put.chunks.len())
        .map_err(|_| SyncError::Protocol("too many declared chunks".to_owned()))?;
    let mut out =
        Vec::with_capacity(LINK_PUT_PREFIX_LEN + put.chunks.len() * 32 + put.publication.len());
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(put.feed.as_bytes());
    out.push(if put.expires_at.is_some() {
        LINK_PUT_FLAG_EXPIRES
    } else {
        0
    });
    out.extend_from_slice(&put.expires_at.unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    for chunk in &put.chunks {
        out.extend_from_slice(chunk.as_bytes());
    }
    out.extend_from_slice(&put.publication);
    Ok(out)
}

/// Parses [`encode_link_put`] output strictly: truncation, an unknown flag bit, an `expires_at` that
/// is not 0 when the flag is clear, a count the input cannot hold, an empty publication and a v1
/// version byte are errors (never a panic whatever the bytes); more than [`LINK_MAX_CHUNKS`] declared
/// chunks is [`SyncError::Protocol`]. The publication's own envelope is not looked at here
/// ([`peek_publication`]).
pub fn decode_link_put(bytes: &[u8]) -> Result<LinkPut, SyncError> {
    let mut r = Reader::new(bytes, "link upload");
    read_version(&mut r)?;
    let feed = read_feed(&mut r)?;
    let flags = r.u8("flags")?;
    if flags & !LINK_PUT_FLAG_EXPIRES != 0 {
        return Err(SyncError::Encoding(
            "link upload: unknown flag bits".to_owned(),
        ));
    }
    let expires = r.i64("expiry")?;
    let expires_at = if flags & LINK_PUT_FLAG_EXPIRES != 0 {
        Some(expires)
    } else if expires == 0 {
        None
    } else {
        return Err(SyncError::Encoding(
            "link upload: an expiry without its flag".to_owned(),
        ));
    };
    let count = r.u32("chunk count")?;
    let within_limit = usize::try_from(count).is_ok_and(|count| count <= LINK_MAX_CHUNKS);
    if !within_limit {
        return Err(SyncError::Protocol(format!(
            "a link declares at most {LINK_MAX_CHUNKS} chunks"
        )));
    }
    let count = r.fit_count(count, "chunk count", 32)?;
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        chunks.push(ChunkId::from_bytes(r.array("chunk hash")?));
    }
    let publication = r.take(r.remaining(), "publication")?.to_vec();
    r.finish()?;
    if publication.is_empty() {
        return Err(SyncError::Encoding(
            "link upload: the publication is missing".to_owned(),
        ));
    }
    Ok(LinkPut {
        feed,
        expires_at,
        chunks,
        publication,
    })
}

/// What `GET /v2/links/{id}` answers (`RemoteStore::link_get`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkFetch {
    /// The link's current publication and its `pub_seq` (the `ETag`).
    Publication {
        /// The server's counter for the link: the `If-None-Match` of the next poll.
        pub_seq: u64,
        /// The sealed publication.
        bytes: Vec<u8>,
    },
    /// The publication is still the one with the `pub_seq` the caller sent (304).
    NotModified,
}

#[cfg(test)]
mod tests;
