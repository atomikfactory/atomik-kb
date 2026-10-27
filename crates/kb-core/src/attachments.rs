//! Attachments (Milestone 5, ADR-0020): uploads streamed into content-addressed
//! encrypted chunks, manifests in the structure document, keys in the keyring, a derived
//! reference index and retention-based garbage collection.
//!
//! Memory stays constant by construction: an upload buffers at most one plaintext chunk,
//! a read holds one ciphertext chunk and its plaintext, and the blob store handles one
//! chunk per call. Names and MIME types live in the encrypted structure document and in
//! the SQLCipher index, never in logs.

use std::collections::{BTreeSet, HashMap, HashSet};

use kb_crypto::{CHUNK_OVERHEAD, Hasher, Secret32, StreamSealer, bucket_size, open_chunk_at};
use kb_keys::keyring as kr;
use kb_keys::{KeySubject, KeyringItem};
use kb_model::AttachmentManifest;
use kb_storage::{
    AttachmentQueries, AttachmentRow, PagePropertyQueries, SyncQueries, UpdateOrigin,
};
use kb_types::attachments::{
    ATTACHMENT_AAD_DOMAIN, CHUNK_SIZE, MAX_ATTACHMENT_BYTES, MAX_MIME_BYTES, MAX_NAME_BYTES,
    RETENTION_SETTING,
};
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, ByteRange, ChunkId, Event, GcSummary, MemberRole,
    PageId, Request, Response, SpaceId,
};
use serde_json::json;

use crate::properties::PropsScope;
use crate::security::SPACE_EPOCH;
use crate::{Core, documents, map_storage_error, short_id};

/// Receives decrypted plaintext pieces of an attachment, in order; a piece is at most one
/// chunk (`kb_types::attachments::CHUNK_SIZE`) long. Returning an error stops the read.
pub type AttachmentSink<'a> = &'a mut dyn FnMut(&[u8]) -> Result<(), ApiError>;

/// Days of retention when the setting is absent or malformed.
const DEFAULT_RETENTION_DAYS: f64 = kb_types::attachments::DEFAULT_RETENTION_DAYS as f64;
const MS_PER_DAY: f64 = 86_400_000.0;

/// An upload between `AttachmentBegin` and `AttachmentFinish`.
pub(crate) struct Upload {
    space_id: SpaceId,
    name: String,
    mime: String,
    /// The attachment key, kept only until the record is written at `finish`.
    record: kr::KeyRecord,
    sealer: StreamSealer,
    hasher: Hasher,
    /// Plaintext not yet sealed; always shorter than one chunk between calls.
    buffer: Vec<u8>,
    chunks: Vec<ChunkId>,
    written: u64,
    created_at: i64,
}

pub(crate) fn aad_prefix(attachment_id: AttachmentId) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(ATTACHMENT_AAD_DOMAIN.len() + 16);
    prefix.extend_from_slice(ATTACHMENT_AAD_DOMAIN);
    prefix.extend_from_slice(attachment_id.as_bytes());
    prefix
}

/// Validates an attachment name: 1 to 255 bytes, no path separators or NUL, not `.` or
/// `..`, not blank.
pub(crate) fn attachment_name(name: &str) -> Result<String, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::invalid("name", "the attachment needs a name"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(ApiError::invalid(
            "name",
            format!("the name must be at most {MAX_NAME_BYTES} bytes"),
        ));
    }
    if name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(ApiError::invalid(
            "name",
            "the name must not contain path separators",
        ));
    }
    Ok(name.to_owned())
}

/// Validates a MIME type: 1 to 127 bytes of printable ASCII with exactly one `/`, neither
/// at an end.
pub(crate) fn mime_type(mime: &str) -> Result<String, ApiError> {
    let bytes = mime.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_MIME_BYTES {
        return Err(ApiError::invalid(
            "mime",
            format!("the MIME type must be 1 to {MAX_MIME_BYTES} bytes"),
        ));
    }
    if !bytes.iter().all(|b| (0x21..=0x7e).contains(b)) {
        return Err(ApiError::invalid(
            "mime",
            "the MIME type must be printable ASCII",
        ));
    }
    let slashes = bytes.iter().filter(|b| **b == b'/').count();
    if slashes != 1 || bytes[0] == b'/' || bytes[bytes.len() - 1] == b'/' {
        return Err(ApiError::invalid(
            "mime",
            "the MIME type must look like type/subtype",
        ));
    }
    Ok(mime.to_owned())
}

/// Pads the final chunk: a `u32 LE` length, the bytes, then random bytes up to the size
/// bucket of `kb_crypto::bucket_size`, capped so the chunk never exceeds `CHUNK_SIZE` plus
/// the length prefix (a nearly full final chunk would otherwise round up to 8 MiB).
fn pad_final(plain: &[u8], rng: &dyn kb_types::Rng) -> Vec<u8> {
    let min = plain.len() + FINAL_PREFIX;
    let target = bucket_size(min).min(CHUNK_SIZE + FINAL_PREFIX).max(min);
    let mut out = Vec::with_capacity(target);
    out.extend_from_slice(&u32::try_from(plain.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(plain);
    let mut padding = vec![0u8; target - min];
    rng.fill_bytes(&mut padding);
    out.extend_from_slice(&padding);
    out
}

/// Inverse of [`pad_final`].
fn unpad_final(padded: &[u8]) -> Option<&[u8]> {
    let (prefix, rest) = padded.split_first_chunk::<FINAL_PREFIX>()?;
    let len = u32::from_le_bytes(*prefix) as usize;
    rest.get(..len)
}

/// Bytes of the final chunk's length prefix.
const FINAL_PREFIX: usize = 4;

/// Chunks an attachment of `size` bytes has: one per full chunk, plus the final chunk
/// (always present, padded, possibly holding no bytes at all).
fn chunk_count(size: u64) -> u32 {
    let chunk = CHUNK_SIZE as u64;
    u32::try_from(size / chunk + 1).unwrap_or(u32::MAX)
}

fn missing_chunk(id: &ChunkId) -> ApiError {
    ApiError::storage(format!(
        "an attachment chunk is missing from the store ({})",
        &id.to_hex()[..8]
    ))
}

fn damaged() -> ApiError {
    ApiError::crypto("the attachment is damaged or its key is wrong")
}

fn not_found() -> ApiError {
    ApiError::not_found("attachment not found")
}

impl Core {
    /// Routes the attachment requests.
    pub(crate) fn dispatch_attachments(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::AttachmentBegin {
                space_id,
                name,
                mime,
            } => Ok(Response::AttachmentStarted(
                self.attachment_begin(space_id, &name, &mime)?,
            )),
            Request::AttachmentFinish { attachment_id } => {
                Ok(Response::Attachment(self.attachment_finish(attachment_id)?))
            }
            Request::AttachmentAbort { attachment_id } => {
                self.attachment_abort(attachment_id)?;
                Ok(Response::Ok)
            }
            Request::AttachmentList { space_id } => {
                Ok(Response::Attachments(self.attachment_list(space_id)?))
            }
            Request::AttachmentGet { attachment_id } => {
                Ok(Response::Attachment(self.attachment_info(attachment_id)?))
            }
            Request::AttachmentDelete { attachment_id } => {
                // Deleting a file (a tombstone, a chunk delete) is Owner only in a shared space.
                if let Some(row) = self
                    .db
                    .attachment_get(attachment_id)
                    .map_err(map_storage_error)?
                {
                    self.require_role(row.space_id, MemberRole::Owner)?;
                }
                let now = self.clock.now_ms();
                self.purge_attachment(attachment_id, now)?;
                Ok(Response::Ok)
            }
            Request::AttachmentGc => {
                let now = self.clock.now_ms();
                Ok(Response::AttachmentGc(self.attachment_gc(now)?))
            }
            Request::AttachmentDownload { attachment_id } => Ok(Response::Attachment(
                self.attachment_download(attachment_id)?,
            )),
            other => Err(ApiError::internal(format!(
                "{} is not an attachment request",
                other.kind()
            ))),
        }
    }

    // ----- upload -----------------------------------------------------------------

    /// Starts an upload: validates the name and MIME type, allocates the id and the key.
    pub(crate) fn attachment_begin(
        &mut self,
        space_id: SpaceId,
        name: &str,
        mime: &str,
    ) -> Result<AttachmentId, ApiError> {
        let name = attachment_name(name)?;
        let mime = mime_type(mime)?;
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_none()
        {
            return Err(ApiError::invalid("workspace", "create the workspace first"));
        }
        if !self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::not_found("space not found"));
        }
        self.require_role(space_id, MemberRole::Editor)?;
        let now = self.clock.now_ms();
        let attachment_id = AttachmentId::generate(self.rng.as_ref());
        if self.uploads.contains_key(&attachment_id)
            || self
                .db
                .attachment_get(attachment_id)
                .map_err(map_storage_error)?
                .is_some()
        {
            return Err(ApiError::already_exists("attachment id already in use"));
        }
        let (space_key, wrap_epoch) = self.attachment_wrap_key(space_id)?;
        let (ak, record) = kr::new_attachment_key(
            &space_key,
            space_id,
            attachment_id,
            wrap_epoch,
            &self.security.device_keys,
            self.device.device_id,
            now,
            self.rng.as_ref(),
        );
        self.uploads.insert(
            attachment_id,
            Upload {
                space_id,
                name,
                mime,
                record,
                sealer: StreamSealer::new(ak, aad_prefix(attachment_id)),
                hasher: Hasher::new(),
                buffer: Vec::new(),
                chunks: Vec::new(),
                written: 0,
                created_at: now,
            },
        );
        Ok(attachment_id)
    }

    /// Appends plaintext to an upload started with `AttachmentBegin`; full chunks are
    /// sealed and stored as they complete, so a piece of any size is fine (the shell
    /// sends at most one chunk per call). `NotFound` for an unknown upload,
    /// `InvalidInput` (field `size`) past the attachment size limit.
    pub fn attachment_write(
        &mut self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let upload = self
            .uploads
            .get_mut(&attachment_id)
            .ok_or_else(|| ApiError::not_found("no upload in progress for this attachment"))?;
        let total = upload.written.saturating_add(bytes.len() as u64);
        if total > MAX_ATTACHMENT_BYTES {
            return Err(ApiError::invalid(
                "size",
                format!("an attachment is at most {MAX_ATTACHMENT_BYTES} bytes"),
            )
            .with_details(json!({ "field": "size", "limit": MAX_ATTACHMENT_BYTES })));
        }
        upload.hasher.update(bytes);
        upload.written = total;
        upload.buffer.extend_from_slice(bytes);
        while upload.buffer.len() >= CHUNK_SIZE {
            let rest = upload.buffer.split_off(CHUNK_SIZE);
            let sealed = upload
                .sealer
                .seal_chunk(&upload.buffer, false, self.rng.as_ref())
                .map_err(|e| ApiError::crypto(format!("cannot seal a chunk: {e}")))?;
            let id = ChunkId::from_bytes(kb_crypto::hash(&sealed));
            self.blob_store
                .put(&id, &sealed)
                .map_err(map_storage_error)?;
            upload.chunks.push(id);
            upload.buffer = rest;
        }
        Ok(())
    }

    /// Seals the final chunk and records the attachment: keyring record, manifest in the
    /// structure document, index row. Emits `AttachmentsChanged`.
    pub(crate) fn attachment_finish(
        &mut self,
        attachment_id: AttachmentId,
    ) -> Result<AttachmentInfo, ApiError> {
        // The role may have changed since the upload began: a demoted member's upload is dropped.
        if let Some(space_id) = self
            .uploads
            .get(&attachment_id)
            .map(|upload| upload.space_id)
            && let Err(error) = self.require_role(space_id, MemberRole::Editor)
        {
            if let Some(upload) = self.uploads.remove(&attachment_id) {
                self.drop_chunks(&upload.chunks);
            }
            return Err(error);
        }
        let mut upload = self
            .uploads
            .remove(&attachment_id)
            .ok_or_else(|| ApiError::not_found("no upload in progress for this attachment"))?;
        let outcome = self.seal_final_chunk(&mut upload);
        if let Err(err) = outcome {
            self.drop_chunks(&upload.chunks);
            return Err(err);
        }
        let now = self.clock.now_ms();
        let manifest = AttachmentManifest {
            v: 1,
            attachment_id,
            name: upload.name.clone(),
            mime: upload.mime.clone(),
            size: upload.written,
            hash: upload.hasher.finalize(),
            chunk_size: u32::try_from(CHUNK_SIZE).unwrap_or(u32::MAX),
            chunks: upload.chunks.clone(),
            created_at: upload.created_at,
            created_by: self.device.device_id,
        };
        let space_id = upload.space_id;
        if let Err(err) = self.record_attachment(space_id, &manifest, upload.record.clone(), now) {
            self.drop_chunks(&upload.chunks);
            return Err(err);
        }
        self.enqueue_upload(attachment_id, &manifest.chunks)?;
        self.events.push(Event::AttachmentsChanged { space_id });
        tracing::info!(
            size = upload.written,
            chunks = manifest.chunks.len(),
            "attachment stored"
        );
        self.attachment_info(attachment_id)
    }

    fn seal_final_chunk(&mut self, upload: &mut Upload) -> Result<(), ApiError> {
        let padded = pad_final(&upload.buffer, self.rng.as_ref());
        upload.buffer.clear();
        let sealed = upload
            .sealer
            .seal_chunk(&padded, true, self.rng.as_ref())
            .map_err(|e| ApiError::crypto(format!("cannot seal the final chunk: {e}")))?;
        let id = ChunkId::from_bytes(kb_crypto::hash(&sealed));
        self.blob_store
            .put(&id, &sealed)
            .map_err(map_storage_error)?;
        upload.chunks.push(id);
        Ok(())
    }

    /// Writes the key record, the manifest and the index row of a finished upload.
    fn record_attachment(
        &mut self,
        space_id: SpaceId,
        manifest: &AttachmentManifest,
        record: kr::KeyRecord,
        now: i64,
    ) -> Result<(), ApiError> {
        self.put_keyring_item(KeyringItem::Record(record))?;
        {
            let open = self.structure_mut(space_id)?;
            open.model
                .attachment_put(manifest)
                .map_err(documents::map_model_error)?;
        }
        self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        // Nothing references a fresh upload until a block is inserted, unless a page
        // already named the id (a block pasted ahead of its upload).
        let referenced = self
            .db
            .attachment_reference_count(manifest.attachment_id)
            .map_err(map_storage_error)?;
        self.db
            .attachment_put(&AttachmentRow {
                attachment_id: manifest.attachment_id,
                space_id,
                name: manifest.name.clone(),
                mime: manifest.mime.clone(),
                size: manifest.size,
                chunk_count: chunk_count_of(manifest),
                created_at: manifest.created_at,
                unreferenced_since: if referenced == 0 { Some(now) } else { None },
            })
            .map_err(map_storage_error)?;
        Ok(())
    }

    /// A copy of a file as a new attachment of `target` under a fresh key (ADR-0047, "space move"): a
    /// new id, a new attachment key wrapped under the target's space key, every chunk opened with the
    /// old key and sealed again, the manifest in the target's structure, the index row, and the upload
    /// of the new chunks. The copy and the original share nothing: purging one never touches the
    /// other. `None` when the file is not complete on this device (a copy needs every chunk) or its
    /// key is not here.
    pub(crate) fn attachment_copy(
        &mut self,
        id: AttachmentId,
        target: SpaceId,
        now: i64,
    ) -> Result<Option<AttachmentId>, ApiError> {
        let Some(row) = self.db.attachment_get(id).map_err(map_storage_error)? else {
            return Ok(None);
        };
        let Some(manifest) = self
            .docs
            .structures
            .get(&row.space_id)
            .and_then(|open| open.model.attachment(id))
        else {
            return Ok(None);
        };
        let Ok(key) = self.attachment_key(row.space_id, id) else {
            return Ok(None);
        };
        for chunk in &manifest.chunks {
            if !self.blob_store.has(chunk).map_err(map_storage_error)? {
                return Ok(None);
            }
        }
        let new_id = AttachmentId::generate(self.rng.as_ref());
        let (target_key, wrap_epoch) = self.attachment_wrap_key(target)?;
        let (new_key, record) = kr::new_attachment_key(
            &target_key,
            target,
            new_id,
            wrap_epoch,
            &self.security.device_keys,
            self.device.device_id,
            now,
            self.rng.as_ref(),
        );
        let old_prefix = aad_prefix(id);
        let mut sealer = StreamSealer::new(new_key, aad_prefix(new_id));
        let last = manifest.chunks.len().saturating_sub(1);
        let mut chunks: Vec<ChunkId> = Vec::with_capacity(manifest.chunks.len());
        for (index, chunk) in manifest.chunks.iter().enumerate() {
            let step = (|| -> Result<ChunkId, ApiError> {
                let sealed = self
                    .blob_store
                    .get(chunk)
                    .map_err(map_storage_error)?
                    .ok_or_else(|| missing_chunk(chunk))?;
                if ChunkId::from_bytes(kb_crypto::hash(&sealed)) != *chunk {
                    return Err(damaged());
                }
                let is_final = index == last;
                let plain = open_chunk_at(&key, &old_prefix, index as u64, &sealed, is_final)
                    .map_err(|_| damaged())?;
                let resealed = sealer
                    .seal_chunk(&plain, is_final, self.rng.as_ref())
                    .map_err(|e| ApiError::crypto(format!("cannot seal a chunk: {e}")))?;
                let id = ChunkId::from_bytes(kb_crypto::hash(&resealed));
                self.blob_store
                    .put(&id, &resealed)
                    .map_err(map_storage_error)?;
                Ok(id)
            })();
            match step {
                Ok(new_chunk) => chunks.push(new_chunk),
                Err(error) => {
                    self.drop_chunks(&chunks);
                    return Err(error);
                }
            }
        }
        let copy = AttachmentManifest {
            attachment_id: new_id,
            chunks: chunks.clone(),
            created_by: self.device.device_id,
            ..manifest
        };
        if let Err(error) = self.record_attachment(target, &copy, record, now) {
            self.drop_chunks(&chunks);
            return Err(error);
        }
        self.enqueue_upload(new_id, &chunks)?;
        self.events
            .push(Event::AttachmentsChanged { space_id: target });
        Ok(Some(new_id))
    }

    /// Drops an upload in progress and the chunks it wrote.
    pub(crate) fn attachment_abort(&mut self, attachment_id: AttachmentId) -> Result<(), ApiError> {
        let upload = self
            .uploads
            .remove(&attachment_id)
            .ok_or_else(|| ApiError::not_found("no upload in progress for this attachment"))?;
        self.drop_chunks(&upload.chunks);
        self.events.push(Event::AttachmentsChanged {
            space_id: upload.space_id,
        });
        Ok(())
    }

    /// Deletes chunks, logging rather than failing: the orphan sweep catches leftovers.
    fn drop_chunks(&mut self, chunks: &[ChunkId]) -> u64 {
        let mut freed = 0u64;
        for id in chunks {
            match self.blob_store.get(id) {
                Ok(Some(bytes)) => freed += bytes.len() as u64,
                Ok(None) => {}
                Err(err) => tracing::warn!(code = "blob", "cannot size a chunk: {err}"),
            }
            if let Err(err) = self.blob_store.delete(id) {
                tracing::warn!("cannot delete a chunk: {err}");
            }
        }
        freed
    }

    // ----- read ---------------------------------------------------------------------

    /// Streams an attachment's plaintext into `sink`, whole or the half-open byte
    /// `range`, decrypting one chunk at a time; answers the attachment's info. `NotFound`
    /// for an unknown or purged attachment, `InvalidInput` (field `range`) for a range
    /// outside the file, `Storage` for a missing chunk, `Crypto` for a chunk that does
    /// not open.
    pub fn attachment_read(
        &mut self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        let info = self.attachment_info(attachment_id)?;
        self.require_complete(&info)?;
        let manifest = self
            .structure(info.space_id)?
            .model
            .attachment(attachment_id)
            .ok_or_else(not_found)?;
        let range = match range {
            Some(range) => {
                if range.start > range.end || range.end > manifest.size {
                    return Err(ApiError::invalid(
                        "range",
                        "the byte range is outside the attachment",
                    ));
                }
                range
            }
            None => ByteRange {
                start: 0,
                end: manifest.size,
            },
        };
        let key = self.attachment_key(info.space_id, attachment_id)?;
        let prefix = aad_prefix(attachment_id);
        let chunk_size = u64::from(manifest.chunk_size.max(1));
        let last = manifest.chunks.len().saturating_sub(1);
        let full_read = range.start == 0 && range.end == manifest.size;
        let first_chunk = usize::try_from(range.start / chunk_size).unwrap_or(usize::MAX);
        let last_needed = if range.end == 0 {
            0
        } else {
            usize::try_from((range.end - 1) / chunk_size).unwrap_or(usize::MAX)
        };
        let mut hasher = Hasher::new();
        let mut delivered = 0u64;
        for index in first_chunk..=last_needed.min(last) {
            let id = manifest.chunks.get(index).ok_or_else(damaged)?;
            let sealed = self
                .blob_store
                .get(id)
                .map_err(map_storage_error)?
                .ok_or_else(|| missing_chunk(id))?;
            if ChunkId::from_bytes(kb_crypto::hash(&sealed)) != *id {
                return Err(damaged());
            }
            let is_final = index == last;
            let plain = open_chunk_at(&key, &prefix, index as u64, &sealed, is_final)
                .map_err(|_| damaged())?;
            let plain: &[u8] = if is_final {
                unpad_final(&plain).ok_or_else(damaged)?
            } else {
                &plain
            };
            let chunk_start = index as u64 * chunk_size;
            let chunk_end = chunk_start + plain.len() as u64;
            if chunk_end > manifest.size || (!is_final && plain.len() as u64 != chunk_size) {
                return Err(damaged());
            }
            if full_read {
                hasher.update(plain);
            }
            let from = range
                .start
                .saturating_sub(chunk_start)
                .min(plain.len() as u64) as usize;
            let to = range
                .end
                .saturating_sub(chunk_start)
                .min(plain.len() as u64) as usize;
            if from < to {
                sink(&plain[from..to])?;
                delivered += (to - from) as u64;
            }
        }
        // The chunks must back every byte the manifest promises in the range.
        if delivered != range.end - range.start {
            return Err(damaged());
        }
        if full_read && hasher.finalize() != manifest.hash {
            return Err(damaged());
        }
        Ok(info)
    }

    /// The key and epoch a new attachment record of a space is wrapped under: epoch 0 for a personal
    /// space (attachments never rotate, ADR-0022), the feed's current epoch for a shared one (ADR-0044:
    /// members never hold the keys of the epochs before the space was shared).
    pub(crate) fn attachment_wrap_key(
        &mut self,
        space_id: SpaceId,
    ) -> Result<(Secret32, u32), ApiError> {
        if self.shared_feed_of(space_id)?.is_some() {
            let epoch = self.space_epoch(space_id);
            let key = self.space_key_at(space_id, epoch)?.ok_or_else(|| {
                ApiError::crypto("this device does not hold the space key of the current epoch")
            })?;
            return Ok((key, epoch));
        }
        Ok((self.attachment_space_key(space_id)?, SPACE_EPOCH))
    }

    /// Attachment keys are wrapped under the space key of epoch 0 whatever the
    /// current epoch (attachments never rotate, ADR-0022); the first record of a
    /// space is that epoch's.
    pub(crate) fn attachment_space_key(&mut self, space_id: SpaceId) -> Result<Secret32, ApiError> {
        self.ensure_space_key(space_id)?;
        self.space_key_at(space_id, SPACE_EPOCH)?
            .ok_or_else(|| ApiError::internal("the space has no first-epoch key"))
    }

    /// The unwrapped attachment key from the keyring: the record of `space_id`, or, when that
    /// space has none (an attachment that moved with its pages, ADR-0038, before the moving
    /// device's record arrived), the record of another space that holds one. Every record of an
    /// attachment wraps the same key.
    pub(crate) fn attachment_key(
        &mut self,
        space_id: SpaceId,
        attachment_id: AttachmentId,
    ) -> Result<Secret32, ApiError> {
        let subject = KeySubject::Attachment { attachment_id };
        if self.security.keyring.is_tombstoned(space_id, &subject) {
            return Err(not_found());
        }
        let mut spaces = vec![space_id];
        spaces.extend(
            self.security
                .keyring
                .attachment_spaces(attachment_id)
                .into_iter()
                .filter(|space| *space != space_id),
        );
        let mut found = false;
        for space in spaces {
            let Some(record) = self.security.keyring.record(space, &subject).cloned() else {
                continue;
            };
            found = true;
            let space_key = if self.shared_feed_of(space)?.is_some() {
                // A shared space's records are wrapped under the epoch that was current when they
                // were made: the member holds the keys from the space's first shared epoch on.
                match self.space_key_at(space, record.wrap_epoch) {
                    Ok(Some(key)) => key,
                    _ => continue,
                }
            } else {
                let Ok(key) = self.attachment_space_key(space) else {
                    continue;
                };
                key
            };
            if let Ok(key) = kr::unwrap_attachment_key(&space_key, &record) {
                return Ok(key);
            }
        }
        if found {
            Err(damaged())
        } else {
            Err(ApiError::crypto("the attachment has no key record"))
        }
    }

    // ----- listing ------------------------------------------------------------------

    pub(crate) fn attachment_list(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<AttachmentInfo>, ApiError> {
        if !self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::not_found("space not found"));
        }
        self.db
            .attachments_by_space(space_id)
            .map_err(map_storage_error)?
            .iter()
            .map(|row| self.info_of(row))
            .collect()
    }

    pub(crate) fn attachment_info(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<AttachmentInfo, ApiError> {
        let row = self
            .db
            .attachment_get(attachment_id)
            .map_err(map_storage_error)?
            .ok_or_else(not_found)?;
        self.info_of(&row)
    }

    fn info_of(&self, row: &AttachmentRow) -> Result<AttachmentInfo, ApiError> {
        let (local_chunks, availability) = self.availability_of(row)?;
        Ok(AttachmentInfo {
            attachment_id: row.attachment_id,
            space_id: row.space_id,
            name: row.name.clone(),
            mime: row.mime.clone(),
            size: row.size,
            chunk_count: row.chunk_count,
            created_at: row.created_at,
            references: self
                .db
                .attachment_reference_count(row.attachment_id)
                .map_err(map_storage_error)?,
            unreferenced_since: row.unreferenced_since,
            local_chunks,
            availability,
        })
    }

    // ----- references -----------------------------------------------------------------

    /// After the references of some attachments may have changed: sets or clears
    /// `unreferenced_since` where the count crossed zero and announces the spaces whose
    /// attachments changed.
    pub(crate) fn attachment_refs_changed(
        &mut self,
        ids: impl IntoIterator<Item = AttachmentId>,
        now: i64,
    ) -> Result<(), ApiError> {
        let mut spaces = BTreeSet::new();
        for id in ids {
            let Some(row) = self.db.attachment_get(id).map_err(map_storage_error)? else {
                continue;
            };
            let count = self
                .db
                .attachment_reference_count(id)
                .map_err(map_storage_error)?;
            let next = match (count, row.unreferenced_since) {
                (0, None) => Some(Some(now)),
                (n, Some(_)) if n > 0 => Some(None),
                _ => None,
            };
            if let Some(value) = next {
                self.db
                    .attachment_set_unreferenced_since(id, value)
                    .map_err(map_storage_error)?;
                spaces.insert(row.space_id);
            }
        }
        for space_id in spaces {
            self.events.push(Event::AttachmentsChanged { space_id });
        }
        Ok(())
    }

    /// The attachments referenced by `pages`, for the bookkeeping that follows a purge.
    pub(crate) fn attachments_referenced_by(&self, pages: &[PageId]) -> Vec<AttachmentId> {
        let mut ids = BTreeSet::new();
        for page in pages {
            if let Ok(found) = self.db.attachment_ids_referenced_by(*page) {
                ids.extend(found);
            }
        }
        ids.into_iter().collect()
    }

    // ----- purge and garbage collection -----------------------------------------------

    /// Purges an attachment now: key tombstone, manifest removed, chunks deleted, index
    /// rows gone. Returns the ciphertext bytes freed.
    pub(crate) fn purge_attachment(
        &mut self,
        attachment_id: AttachmentId,
        now: i64,
    ) -> Result<(u32, u64), ApiError> {
        let row = self
            .db
            .attachment_get(attachment_id)
            .map_err(map_storage_error)?
            .ok_or_else(not_found)?;
        let space_id = row.space_id;
        // An attachment that moved with its pages (ADR-0038) has a key record in every space it
        // lived in, and until a remote move settles, possibly a manifest in two structures.
        let mut spaces: BTreeSet<SpaceId> = self
            .security
            .keyring
            .attachment_spaces(attachment_id)
            .into_iter()
            .collect();
        spaces.insert(space_id);
        let holders: Vec<SpaceId> = self
            .docs
            .structures
            .iter()
            .filter(|(_, open)| open.model.attachment(attachment_id).is_some())
            .map(|(space, _)| *space)
            .collect();
        let chunks: Vec<ChunkId> = self
            .docs
            .structures
            .get(&space_id)
            .and_then(|open| open.model.attachment(attachment_id))
            .or_else(|| {
                holders.iter().find_map(|space| {
                    self.docs
                        .structures
                        .get(space)?
                        .model
                        .attachment(attachment_id)
                })
            })
            .map(|m| m.chunks)
            .unwrap_or_default();
        // Crypto-erase first: even a chunk that survives a failed delete is unreadable.
        let subject = KeySubject::Attachment { attachment_id };
        for space in spaces {
            if !self.security.keyring.is_tombstoned(space, &subject) {
                let tombstone = kr::tombstone(
                    space,
                    subject,
                    &self.security.device_keys,
                    self.device.device_id,
                    now,
                );
                self.put_keyring_item(KeyringItem::Tombstone(tombstone))?;
            }
        }
        for space in holders {
            if let Some(open) = self.docs.structures.get_mut(&space) {
                open.model
                    .attachment_remove(attachment_id)
                    .map_err(documents::map_model_error)?;
                self.persist_structure(space, Some(&[]), UpdateOrigin::Local)?;
            }
        }
        let freed = self.drop_chunks(&chunks);
        // The chunks of a shared space are deleted from its feed's namespace, which the index row
        // (gone in a moment) is what tells.
        if let Some(row) = self.shared_feed_of(space_id)? {
            self.db
                .sync_config_set(
                    &crate::feeds::chunk_delete_key(attachment_id),
                    &row.feed.to_string(),
                )
                .map_err(map_storage_error)?;
        }
        self.enqueue_delete(attachment_id, &chunks)?;
        // The pages whose property values attach it: their text rows hold its file name.
        let attached_by: Vec<PageId> = self
            .db
            .page_properties_pages_with_attachment(attachment_id)
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|(space, _)| *space == space_id)
            .map(|(_, page)| page)
            .collect();
        self.db
            .attachment_delete(attachment_id)
            .map_err(map_storage_error)?;
        if !attached_by.is_empty() && self.docs.structures.contains_key(&space_id) {
            // The manifest is gone, so the name is no longer searchable (the rows stay: the
            // value keeps naming the file, which then shows as deleted).
            self.refresh_property_rows(space_id, PropsScope::Pages(&attached_by))?;
        }
        self.events.push(Event::AttachmentsChanged { space_id });
        tracing::info!(chunks = chunks.len(), freed, "attachment purged");
        Ok((u32::try_from(chunks.len()).unwrap_or(u32::MAX), freed))
    }

    /// Purges every attachment of a space (the space is being deleted).
    pub(crate) fn purge_space_attachments(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let rows = self
            .db
            .attachments_by_space(space_id)
            .map_err(map_storage_error)?;
        for row in rows {
            self.purge_attachment(row.attachment_id, now)?;
        }
        Ok(())
    }

    /// Retention in milliseconds from the setting; `None` = keep forever.
    fn retention_ms(&self) -> Result<Option<i64>, ApiError> {
        let days = match self
            .db
            .setting_get(RETENTION_SETTING)
            .map_err(map_storage_error)?
        {
            Some(value) => value.as_f64().unwrap_or(DEFAULT_RETENTION_DAYS),
            None => DEFAULT_RETENTION_DAYS,
        };
        if days <= 0.0 || !days.is_finite() {
            return Ok(None);
        }
        // Saturating float-to-int conversion is what `as` does.
        Ok(Some((days * MS_PER_DAY) as i64))
    }

    /// Collects attachments unreferenced for longer than the retention setting and
    /// sweeps chunks no manifest and no upload claims.
    pub(crate) fn attachment_gc(&mut self, now: i64) -> Result<GcSummary, ApiError> {
        let mut summary = GcSummary::default();
        if let Some(retention) = self.retention_ms()? {
            let cutoff = now.saturating_sub(retention);
            let rows = self
                .db
                .attachments_unreferenced_before(cutoff)
                .map_err(map_storage_error)?;
            for row in rows {
                // A shared space's files are collected by an Owner's device only (ADR-0047).
                if !self.may_purge_in(row.space_id) {
                    continue;
                }
                let (chunks, freed) = self.purge_attachment(row.attachment_id, now)?;
                summary.purged += 1;
                summary.chunks_deleted += chunks;
                summary.bytes_freed += freed;
            }
        }
        let (chunks, freed) = self.sweep_orphan_chunks()?;
        summary.chunks_deleted += chunks;
        summary.bytes_freed += freed;
        if summary.purged > 0 || summary.chunks_deleted > 0 {
            tracing::info!(
                purged = summary.purged,
                chunks = summary.chunks_deleted,
                freed = summary.bytes_freed,
                "attachments collected"
            );
        }
        Ok(summary)
    }

    /// Deletes chunks that no manifest and no upload in progress claims, and the
    /// leftovers of interrupted writes.
    fn sweep_orphan_chunks(&mut self) -> Result<(u32, u64), ApiError> {
        let leftovers = self
            .blob_store
            .sweep_temporaries()
            .map_err(map_storage_error)?;
        if leftovers > 0 {
            tracing::info!(leftovers, "removed interrupted chunk writes");
        }
        let mut claimed: HashSet<ChunkId> = HashSet::new();
        for open in self.docs.structures.values() {
            for manifest in open.model.attachments() {
                claimed.extend(manifest.chunks);
            }
        }
        for upload in self.uploads.values() {
            claimed.extend(upload.chunks.iter().copied());
        }
        let orphans: Vec<ChunkId> = self
            .blob_store
            .list()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|id| !claimed.contains(id))
            .collect();
        let freed = self.drop_chunks(&orphans);
        Ok((u32::try_from(orphans.len()).unwrap_or(u32::MAX), freed))
    }

    // ----- open-time repair -------------------------------------------------------------

    /// Makes the attachment index agree with the manifests at open and after a restore: missing rows
    /// are added, rows without a manifest are dropped, and the references of a repaired space are
    /// recomputed from its page documents. A difference found here is a repair, and says so.
    pub(crate) fn verify_attachment_index(&mut self) -> Result<(), ApiError> {
        self.reconcile_attachment_index(true)
    }

    /// The same, for what a remote update of a structure document brought (an attachment that
    /// arrived through a feed, one that a device removed or moved): the index follows the manifests
    /// as they arrive, which is the normal way for the rows of another device's attachments to be
    /// written, so there is nothing to warn about.
    pub(crate) fn attachments_arrived(&mut self) -> Result<(), ApiError> {
        self.reconcile_attachment_index(false)
    }

    fn reconcile_attachment_index(&mut self, repair: bool) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let spaces: Vec<SpaceId> = self.docs.structures.keys().copied().collect();
        // Which structures hold each manifest: an attachment that moved with its pages
        // (ADR-0038) leaves one structure and arrives in another, in either order, and may
        // briefly be in both.
        let mut holders: HashMap<AttachmentId, Vec<SpaceId>> = HashMap::new();
        for (space_id, open) in &self.docs.structures {
            for manifest in open.model.attachments() {
                holders
                    .entry(manifest.attachment_id)
                    .or_default()
                    .push(*space_id);
            }
        }
        for space_id in spaces {
            // The image icons of the pages (ADR-0029): compared with the documents whatever the
            // manifests say, so a database migrated to schema v14 gets its rows here and a damaged
            // table is repaired.
            let icon_changes = self.refresh_icon_refs(space_id)?;
            if !icon_changes.is_empty() {
                self.attachment_refs_changed(icon_changes, now)?;
            }
            let manifests: HashMap<AttachmentId, AttachmentManifest> = self
                .structure(space_id)?
                .model
                .attachments()
                .into_iter()
                .map(|m| (m.attachment_id, m))
                .collect();
            let rows = self
                .db
                .attachments_by_space(space_id)
                .map_err(map_storage_error)?;
            let indexed: HashSet<AttachmentId> = rows.iter().map(|r| r.attachment_id).collect();
            let mut wanted: HashSet<AttachmentId> = manifests.keys().copied().collect();
            if indexed == wanted {
                continue;
            }
            // A manifest another structure holds too, whose index row names that space, is that
            // space's to index (the duplicate is settled by `reconcile_moves`).
            let mut owned_elsewhere: HashSet<AttachmentId> = HashSet::new();
            for id in wanted.difference(&indexed) {
                if let Some(row) = self.db.attachment_get(*id).map_err(map_storage_error)?
                    && row.space_id != space_id
                    && holders
                        .get(id)
                        .is_some_and(|spaces| spaces.contains(&row.space_id))
                {
                    owned_elsewhere.insert(*id);
                }
            }
            for id in &owned_elsewhere {
                wanted.remove(id);
            }
            if indexed == wanted {
                continue;
            }
            if repair {
                tracing::warn!(space = %short_id(&space_id), indexed = indexed.len(), manifests = wanted.len(), "attachment index out of sync; rebuilding");
            } else {
                tracing::debug!(space = %short_id(&space_id), indexed = indexed.len(), manifests = wanted.len(), "attachment index follows the manifests");
            }
            for stale in indexed.difference(&wanted) {
                // The manifest left this structure for another one (it moved): its row follows
                // it. Only a manifest that is nowhere is gone.
                let elsewhere = holders
                    .get(stale)
                    .and_then(|spaces| spaces.iter().copied().find(|space| *space != space_id));
                if let Some(space) = elsewhere {
                    self.db
                        .attachment_set_space(*stale, space)
                        .map_err(map_storage_error)?;
                    self.events.push(Event::AttachmentsChanged { space_id });
                    self.events
                        .push(Event::AttachmentsChanged { space_id: space });
                    continue;
                }
                self.db
                    .attachment_delete(*stale)
                    .map_err(map_storage_error)?;
                self.forget_attachment_chunks(*stale)?;
            }
            for id in wanted.difference(&indexed) {
                let manifest = &manifests[id];
                // A row that names a space without this manifest follows it here.
                if let Some(row) = self.db.attachment_get(*id).map_err(map_storage_error)? {
                    self.db
                        .attachment_set_space(*id, space_id)
                        .map_err(map_storage_error)?;
                    self.events.push(Event::AttachmentsChanged {
                        space_id: row.space_id,
                    });
                    self.events.push(Event::AttachmentsChanged { space_id });
                    continue;
                }
                self.db
                    .attachment_put(&AttachmentRow {
                        attachment_id: *id,
                        space_id,
                        name: manifest.name.clone(),
                        mime: manifest.mime.clone(),
                        size: manifest.size,
                        chunk_count: chunk_count_of(manifest),
                        created_at: manifest.created_at,
                        unreferenced_since: None,
                    })
                    .map_err(map_storage_error)?;
            }
            self.rebuild_space_text_index(space_id)?;
            let ids: Vec<AttachmentId> = wanted.into_iter().collect();
            self.attachment_refs_changed(ids, now)?;
        }
        // Chunks of manifests that arrived or came back get their transfer rows.
        self.reconcile_chunks()?;
        Ok(())
    }
}

fn chunk_count_of(manifest: &AttachmentManifest) -> u32 {
    u32::try_from(manifest.chunks.len()).unwrap_or_else(|_| chunk_count(manifest.size))
}

/// Largest ciphertext a chunk can be: a full chunk, or a final chunk with its length
/// prefix, plus the AEAD overhead.
pub const MAX_SEALED_CHUNK: usize = CHUNK_SIZE + FINAL_PREFIX + CHUNK_OVERHEAD;
