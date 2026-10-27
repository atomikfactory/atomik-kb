//! Attachment chunk transfers (Milestone 8, ADR-0023): the per-chunk state table
//! (`chunk_sync`, schema v8) and the methods the driver calls through `kb-net`'s
//! `SyncHost`.
//!
//! Every chunk of every manifest this device knows has one row: `UPLOAD` (the chunk is
//! here and the server lacks it), `DOWNLOAD` (wanted here; the server should have it),
//! `SYNCED` (both have it), `DELETE` (a purge here asks the server to drop it). The
//! table is derived bookkeeping: `reconcile_chunks` rebuilds the missing rows from the
//! manifests and the blob store, a new server epoch sends every `SYNCED` row back to
//! `UPLOAD` (the puts are idempotent), and a disconnect clears it.

use kb_model::AttachmentManifest;
use kb_storage::{AttachmentQueries, AttachmentRow, ChunkSyncQueries, ChunkSyncRow, SyncQueries};
use kb_types::attachments::{DOWNLOAD_POLICY_ON_DEMAND, DOWNLOAD_POLICY_SETTING};
use kb_types::sync::{TRANSFER_RETRY_BASE_MS, TRANSFER_RETRY_MAX_MS};
use kb_types::{
    ApiError, AttachmentAvailability, AttachmentId, AttachmentInfo, ChunkId, Event, SpaceId,
    TransferInfo,
};

use crate::{Core, map_storage_error};

/// The chunk is here and the server lacks it.
pub(crate) const UPLOAD: u8 = 1;
/// The chunk is wanted here; the server should have it.
pub(crate) const DOWNLOAD: u8 = 2;
/// Both have it.
pub(crate) const SYNCED: u8 = 3;
/// A purge here: the server is asked to drop it.
pub(crate) const DELETE: u8 = 4;
/// The chunk is in the personal namespace and a shared feed's lacks it: the share step and a move
/// into a shared space copy it server-side (`FeedChunkKind::Copy`, ADR-0045).
pub(crate) const COPY: u8 = 5;

/// States under which the chunk is in this device's blob store.
const LOCAL: [u8; 2] = [UPLOAD, SYNCED];

impl Core {
    // ----- policy and bookkeeping ---------------------------------------------------------

    /// Whether every chunk of every manifest is wanted at once (the default), or only
    /// on request.
    fn download_eager(&self) -> Result<bool, ApiError> {
        let value = self
            .db
            .setting_get(DOWNLOAD_POLICY_SETTING)
            .map_err(map_storage_error)?;
        Ok(value.as_ref().and_then(|v| v.as_str()) != Some(DOWNLOAD_POLICY_ON_DEMAND))
    }

    /// Gives every chunk of every manifest without a row one: `UPLOAD` when the blob
    /// store has it, `DOWNLOAD` under the eager policy, nothing under the on-demand
    /// policy. Runs at open, after a remote structure apply, after a restore, when the
    /// policy turns eager, and when a server is configured.
    pub(crate) fn reconcile_chunks(&mut self) -> Result<(), ApiError> {
        let eager = self.download_eager()?;
        let manifests: Vec<(SpaceId, AttachmentManifest)> = self
            .docs
            .structures
            .iter()
            .flat_map(|(space_id, open)| {
                open.model
                    .attachments()
                    .into_iter()
                    .map(move |manifest| (*space_id, manifest))
            })
            .collect();
        let mut added = 0u32;
        let mut announced: Vec<SpaceId> = Vec::new();
        for (space_id, manifest) in manifests {
            let known: std::collections::HashSet<ChunkId> = self
                .db
                .chunk_sync_by_attachment(manifest.attachment_id)
                .map_err(map_storage_error)?
                .into_iter()
                .map(|row| row.hash)
                .collect();
            for chunk in &manifest.chunks {
                if known.contains(chunk) {
                    continue;
                }
                let local = self.blob_store.has(chunk).map_err(map_storage_error)?;
                let state = if local {
                    UPLOAD
                } else if eager {
                    DOWNLOAD
                } else {
                    continue;
                };
                self.db
                    .chunk_sync_put(&ChunkSyncRow {
                        hash: *chunk,
                        attachment_id: manifest.attachment_id,
                        state,
                        attempts: 0,
                        next_at: None,
                    })
                    .map_err(map_storage_error)?;
                added += 1;
                if state == DOWNLOAD && !announced.contains(&space_id) {
                    announced.push(space_id);
                }
            }
        }
        for space_id in announced {
            self.announce_attachment(space_id);
        }
        if added > 0 {
            tracing::debug!(added, "chunk transfers queued");
            self.request_sync()?;
        }
        Ok(())
    }

    /// A finished local upload: every chunk goes to the server.
    pub(crate) fn enqueue_upload(
        &mut self,
        attachment_id: AttachmentId,
        chunks: &[ChunkId],
    ) -> Result<(), ApiError> {
        for chunk in chunks {
            self.db
                .chunk_sync_put(&ChunkSyncRow {
                    hash: *chunk,
                    attachment_id,
                    state: UPLOAD,
                    attempts: 0,
                    next_at: None,
                })
                .map_err(map_storage_error)?;
        }
        if !chunks.is_empty() {
            self.request_sync()?;
        }
        Ok(())
    }

    /// A purge here: the server is asked to drop every chunk of the manifest, whatever
    /// the rows said before.
    pub(crate) fn enqueue_delete(
        &mut self,
        attachment_id: AttachmentId,
        chunks: &[ChunkId],
    ) -> Result<(), ApiError> {
        for chunk in chunks {
            self.db
                .chunk_sync_put(&ChunkSyncRow {
                    hash: *chunk,
                    attachment_id,
                    state: DELETE,
                    attempts: 0,
                    next_at: None,
                })
                .map_err(map_storage_error)?;
        }
        if !chunks.is_empty() {
            self.request_sync()?;
        }
        Ok(())
    }

    /// The manifest left through the structure document (another device purged it):
    /// this device forgets the rows; the local chunks are swept as orphans.
    pub(crate) fn forget_attachment_chunks(
        &mut self,
        attachment_id: AttachmentId,
    ) -> Result<(), ApiError> {
        self.db
            .chunk_sync_delete_attachment(attachment_id)
            .map_err(map_storage_error)
    }

    /// `AttachmentDownload`, or a read of an incomplete attachment: every chunk this
    /// device lacks is wanted now, whatever the policy.
    pub(crate) fn demand_attachment(
        &mut self,
        attachment_id: AttachmentId,
    ) -> Result<(), ApiError> {
        let row = self.attachment_row(attachment_id)?;
        let Some(manifest) = self.manifest_of(&row) else {
            return Ok(());
        };
        let existing: std::collections::HashMap<ChunkId, ChunkSyncRow> = self
            .db
            .chunk_sync_by_attachment(attachment_id)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|row| (row.hash, row))
            .collect();
        let mut wanted = 0u32;
        let mut missing = 0u32;
        for chunk in &manifest.chunks {
            match existing.get(chunk) {
                Some(row) if row.state != DOWNLOAD => continue,
                Some(row) if row.attempts == 0 && row.next_at.is_none() => {
                    missing += 1;
                    continue;
                }
                _ => {}
            }
            if self.blob_store.has(chunk).map_err(map_storage_error)? {
                continue;
            }
            missing += 1;
            self.db
                .chunk_sync_put(&ChunkSyncRow {
                    hash: *chunk,
                    attachment_id,
                    state: DOWNLOAD,
                    attempts: 0,
                    next_at: None,
                })
                .map_err(map_storage_error)?;
            wanted += 1;
        }
        if wanted > 0 {
            // The card changes (not downloaded or waiting, then downloading).
            self.announce_attachment(row.space_id);
        }
        if missing > 0 {
            self.request_sync()?;
        }
        Ok(())
    }

    /// `AttachmentDownload`.
    pub(crate) fn attachment_download(
        &mut self,
        attachment_id: AttachmentId,
    ) -> Result<AttachmentInfo, ApiError> {
        self.demand_attachment(attachment_id)?;
        self.attachment_info(attachment_id)
    }

    /// The server lost everything (a new epoch) or another server is configured:
    /// everything local is uploaded again.
    pub(crate) fn chunk_sync_on_server_change(&mut self) -> Result<(), ApiError> {
        let reset = self
            .db
            .chunk_sync_reset(SYNCED, UPLOAD)
            .map_err(map_storage_error)?;
        if reset > 0 {
            tracing::info!(chunks = reset, "chunks to upload again");
        }
        Ok(())
    }

    /// No server any more: nothing to transfer.
    pub(crate) fn chunk_sync_on_disconnect(&mut self) -> Result<(), ApiError> {
        self.db.chunk_sync_clear().map_err(map_storage_error)
    }

    /// The counts of `SyncInfo`.
    pub(crate) fn transfers_info(&self) -> Result<TransferInfo, ApiError> {
        let uploads = self
            .db
            .chunk_sync_count(UPLOAD)
            .map_err(map_storage_error)?;
        let wanted = self
            .db
            .chunk_sync_count(DOWNLOAD)
            .map_err(map_storage_error)?;
        let waiting = self
            .db
            .chunk_sync_count_waiting()
            .map_err(map_storage_error)?;
        Ok(TransferInfo {
            uploads,
            downloads: wanted.saturating_sub(waiting),
            waiting,
        })
    }

    /// Chunks of the attachment present in the blob store, and what that means for the
    /// user. From the rows when the attachment has any; from the blob store otherwise.
    pub(crate) fn availability_of(
        &self,
        row: &AttachmentRow,
    ) -> Result<(u32, AttachmentAvailability), ApiError> {
        let rows = self
            .db
            .chunk_sync_by_attachment(row.attachment_id)
            .map_err(map_storage_error)?;
        if rows.is_empty() {
            let Some(manifest) = self.manifest_of(row) else {
                return Ok((0, AttachmentAvailability::NotDownloaded));
            };
            let mut local = 0u32;
            for chunk in &manifest.chunks {
                if self.blob_store.has(chunk).map_err(map_storage_error)? {
                    local += 1;
                }
            }
            let availability = if local >= row.chunk_count {
                AttachmentAvailability::Complete
            } else if self.download_eager()? {
                AttachmentAvailability::Downloading
            } else {
                AttachmentAvailability::NotDownloaded
            };
            return Ok((local, availability));
        }
        let local = u32::try_from(rows.iter().filter(|r| LOCAL.contains(&r.state)).count())
            .unwrap_or(u32::MAX);
        let wanted: Vec<&ChunkSyncRow> = rows.iter().filter(|r| r.state == DOWNLOAD).collect();
        let availability = if local >= row.chunk_count {
            AttachmentAvailability::Complete
        } else if wanted.is_empty() {
            AttachmentAvailability::NotDownloaded
        } else if wanted.iter().any(|r| r.attempts > 0) {
            AttachmentAvailability::Waiting
        } else {
            AttachmentAvailability::Downloading
        };
        Ok((local.min(row.chunk_count), availability))
    }

    /// `Storage` unless every chunk is here; a read of an incomplete attachment is a
    /// demand under any policy.
    pub(crate) fn require_complete(&mut self, info: &AttachmentInfo) -> Result<(), ApiError> {
        if info.local_chunks >= info.chunk_count {
            return Ok(());
        }
        self.demand_attachment(info.attachment_id)?;
        Err(ApiError::storage(format!(
            "the attachment is not complete on this device ({} of {} chunks)",
            info.local_chunks, info.chunk_count
        )))
    }

    // ----- the chunk host methods (`sync_feed_chunk_*` in `feeds.rs` call them) -------------

    /// The sealed chunk to upload, from the blob store, verified against its id. A
    /// chunk that is not here any more (or is damaged) is wanted instead (eager) or
    /// forgotten (on demand); `None` then.
    pub(crate) fn chunk_upload_bytes(
        &mut self,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        if let Some(bytes) = self.blob_store.get(&hash).map_err(map_storage_error)? {
            if kb_crypto::hash(&bytes) == hash.to_bytes() {
                return Ok(Some(bytes));
            }
            // A damaged local chunk would be refused by the server: dropped here, and
            // fetched again if the policy wants it.
            tracing::warn!(
                chunk = &hash.to_hex()[..8],
                "a local chunk does not hash to its id; dropped"
            );
            self.blob_store.delete(&hash).map_err(map_storage_error)?;
        }
        if let Some(row) = self.db.chunk_sync_get(hash).map_err(map_storage_error)?
            && row.state == UPLOAD
        {
            if self.download_eager()? {
                self.db
                    .chunk_sync_set(hash, DOWNLOAD, 0, None)
                    .map_err(map_storage_error)?;
            } else {
                self.db.chunk_sync_delete(hash).map_err(map_storage_error)?;
            }
        }
        Ok(None)
    }

    /// The server took the chunk. A purge that happened meanwhile keeps its `DELETE`.
    pub(crate) fn chunk_uploaded(&mut self, hash: ChunkId) -> Result<(), ApiError> {
        match self.db.chunk_sync_get(hash).map_err(map_storage_error)? {
            Some(row) if row.state == DELETE => Ok(()),
            Some(_) => self
                .db
                .chunk_sync_set(hash, SYNCED, 0, None)
                .map_err(map_storage_error),
            None => Ok(()),
        }
    }

    /// A chunk arrived: verified against its hash, stored, the attachment's space
    /// announced. A chunk nobody wants any more (purged meanwhile) is dropped.
    pub(crate) fn chunk_received(&mut self, hash: ChunkId, bytes: Vec<u8>) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let Some(row) = self.db.chunk_sync_get(hash).map_err(map_storage_error)? else {
            return Ok(());
        };
        if kb_crypto::hash(&bytes) != hash.to_bytes() {
            self.delay_chunk(&row, now)?;
            return Err(ApiError::crypto(format!(
                "a downloaded chunk does not match its hash ({})",
                &hash.to_hex()[..8]
            )));
        }
        if row.state == DELETE {
            return Ok(());
        }
        let Some(index) = self
            .db
            .attachment_get(row.attachment_id)
            .map_err(map_storage_error)?
        else {
            // Purged by another device meanwhile: nothing claims the chunk.
            self.db.chunk_sync_delete(hash).map_err(map_storage_error)?;
            return Ok(());
        };
        self.blob_store
            .put(&hash, &bytes)
            .map_err(map_storage_error)?;
        self.db
            .chunk_sync_set(hash, SYNCED, 0, None)
            .map_err(map_storage_error)?;
        self.announce_attachment(index.space_id);
        Ok(())
    }

    /// The server does not hold the chunk yet: asked again after a growing delay.
    pub(crate) fn chunk_unavailable(&mut self, hash: ChunkId) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let Some(row) = self.db.chunk_sync_get(hash).map_err(map_storage_error)? else {
            return Ok(());
        };
        if row.state != DOWNLOAD {
            return Ok(());
        }
        self.delay_chunk(&row, now)?;
        if let Some(index) = self
            .db
            .attachment_get(row.attachment_id)
            .map_err(map_storage_error)?
        {
            self.announce_attachment(index.space_id);
        }
        Ok(())
    }

    /// The server no longer holds the chunk.
    pub(crate) fn chunk_deleted(&mut self, hash: ChunkId) -> Result<(), ApiError> {
        let attachment = self
            .db
            .chunk_sync_get(hash)
            .map_err(map_storage_error)?
            .map(|row| row.attachment_id);
        self.db.chunk_sync_delete(hash).map_err(map_storage_error)?;
        // The last chunk of a purged attachment of a shared space is gone: the note of its feed too.
        if let Some(attachment) = attachment
            && self
                .db
                .chunk_sync_by_attachment(attachment)
                .map_err(map_storage_error)?
                .is_empty()
        {
            self.db
                .sync_config_delete(&crate::feeds::chunk_delete_key(attachment))
                .map_err(map_storage_error)?;
        }
        Ok(())
    }

    // ----- helpers ----------------------------------------------------------------------

    fn delay_chunk(&mut self, row: &ChunkSyncRow, now: i64) -> Result<(), ApiError> {
        let attempts = row.attempts.saturating_add(1);
        let delay = TRANSFER_RETRY_BASE_MS
            .saturating_mul(1u64 << attempts.min(20))
            .min(TRANSFER_RETRY_MAX_MS);
        let next_at = now.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX));
        self.db
            .chunk_sync_set(row.hash, row.state, attempts, Some(next_at))
            .map_err(map_storage_error)
    }

    fn announce_attachment(&mut self, space_id: SpaceId) {
        if !self
            .events
            .iter()
            .any(|e| matches!(e, Event::AttachmentsChanged { space_id: s } if *s == space_id))
        {
            self.events.push(Event::AttachmentsChanged { space_id });
        }
    }

    /// Pokes the driver, once per dispatch, when a server is configured.
    pub(crate) fn request_sync(&mut self) -> Result<(), ApiError> {
        if self.sync_server_url()?.is_some()
            && !self
                .events
                .iter()
                .any(|e| matches!(e, Event::SyncRequested))
        {
            self.events.push(Event::SyncRequested);
        }
        Ok(())
    }

    fn attachment_row(&self, attachment_id: AttachmentId) -> Result<AttachmentRow, ApiError> {
        self.db
            .attachment_get(attachment_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::not_found("attachment not found"))
    }

    fn manifest_of(&self, row: &AttachmentRow) -> Option<AttachmentManifest> {
        self.docs
            .structures
            .get(&row.space_id)
            .and_then(|open| open.model.attachment(row.attachment_id))
    }
}
