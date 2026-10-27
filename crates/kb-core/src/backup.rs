//! The encrypted backup file (ADR-0017, `docs/development/milestone-04.md`, "Backup").
//!
//! ```text
//! "KBB1" | u16 LE format = 1 | u8 suite = 1 | u32 LE header_len | header | frames...
//! header: account_id[16] | created_at i64 | recovery_id[32] | recovery_blob[72] | backup_id[16] | chunk_size u32
//! frame:  u8 is_final | u32 LE len | sealed chunk (StreamSealer output)
//! ```
//!
//! The plaintext behind the frames is a sequence of records `u8 tag | u32 LE len |
//! bytes`: workspace (1), setting (2), device log entry (3), keyring item (4),
//! document with a fresh snapshot (5), attachment chunk (6: `chunk_id[32] | sealed
//! chunk`, format 2), end (255). Derived data is not included; the core rebuilds it
//! after a restore. Format 2 added the chunk records; readers accept 1 and 2.
//!
//! The reader streams: records are decoded one at a time from the decrypted frames,
//! metadata is held until the end record, chunks go to the blob store as they arrive
//! (content-addressed and verified; leftovers of a failed restore are swept as orphans).

use kb_crypto::{
    CHUNK_OVERHEAD, CHUNK_SIZE, Label, Secret32, StreamOpener, StreamSealer, WrappedKey,
};
use std::collections::HashSet;

use kb_doc::{Doc, DocKind};
use kb_keys::{Ark, DeviceLog, KeyringItem, KeysError, LogEntry, LogSigner, RecoveryKey};
use kb_model::AccountDoc;
use kb_storage::{
    DeviceLogRow, DocKindCode, DocQueries, DocRow, SecurityQueries, SettingRow, WorkspaceRow,
};
use kb_types::{
    AccountId, ApiError, BackupSummary, ChunkId, DocId, Event, Json, SpaceId, WorkspaceId,
};

/// Settings that belong to this device and to the shell's own dialogs (ADR-0036, ADR-0037): the
/// Ollama endpoint with its consent to send text elsewhere, the folder of scheduled backups and
/// their last outcome. A backup leaves them out and a restore ignores them, so a backup file can
/// never grant a consent or point backups at a folder.
const DEVICE_BOUND_SETTINGS: [&str; 4] = [
    kb_types::ai::AI_ENDPOINT_SETTING,
    kb_types::interop::BACKUP_FOLDER_SETTING,
    kb_types::interop::BACKUP_LAST_SETTING,
    // The Google Drive target (ADR-0059): a backup never carries a connection or its consent.
    kb_types::interop::BACKUP_DRIVE_SETTING,
];

use crate::security::{
    META_ACCOUNT_ID, META_RECOVERY_BLOB, META_RECOVERY_CREATED, META_RECOVERY_ID, hex, keyring_row,
};
use crate::{Core, map_doc_error, map_keys_error, map_storage_error};

/// File magic.
pub const BACKUP_MAGIC: &[u8; 4] = b"KBB1";
/// Format version written. Readers accept every version up to it.
pub const BACKUP_FORMAT: u16 = 2;
/// First format with attachment chunk records.
const FORMAT_WITH_CHUNKS: u16 = 2;
/// Suite id of the stream.
const SUITE: u8 = 1;
/// Plaintext header length without the length prefix.
const HEADER_LEN: usize = 16 + 8 + 32 + 72 + 16 + 4;
/// Largest record the reader accepts (a snapshot of a very large page).
const MAX_RECORD_LEN: u32 = 512 * 1024 * 1024;

const TAG_WORKSPACE: u8 = 1;
const TAG_SETTING: u8 = 2;
const TAG_LOG_ENTRY: u8 = 3;
const TAG_KEYRING: u8 = 4;
const TAG_DOCUMENT: u8 = 5;
const TAG_CHUNK: u8 = 6;
const TAG_END: u8 = 255;

/// The plaintext header.
struct Header {
    account_id: AccountId,
    created_at: i64,
    recovery_id: [u8; 32],
    recovery_blob: WrappedKey,
    backup_id: [u8; 16],
    chunk_size: u32,
}

impl Header {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN);
        out.extend_from_slice(self.account_id.as_bytes());
        out.extend_from_slice(&self.created_at.to_le_bytes());
        out.extend_from_slice(&self.recovery_id);
        out.extend_from_slice(&self.recovery_blob.to_bytes());
        out.extend_from_slice(&self.backup_id);
        out.extend_from_slice(&self.chunk_size.to_le_bytes());
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self, ApiError> {
        if bytes.len() != HEADER_LEN {
            return Err(bad_file("header has the wrong length"));
        }
        let (account, rest) = bytes.split_at(16);
        let (created, rest) = rest.split_at(8);
        let (rid, rest) = rest.split_at(32);
        let (blob, rest) = rest.split_at(72);
        let (bid, rest) = rest.split_at(16);
        let account_id = AccountId::from_bytes(array16(account)?);
        let created_at = i64::from_le_bytes(array8(created)?);
        let recovery_id: [u8; 32] = rid.try_into().map_err(|_| bad_file("recovery id"))?;
        let recovery_blob = WrappedKey::from_bytes(blob).map_err(|_| bad_file("recovery blob"))?;
        let backup_id = array16(bid)?;
        let chunk_size = u32::from_le_bytes(rest.try_into().map_err(|_| bad_file("chunk size"))?);
        Ok(Self {
            account_id,
            created_at,
            recovery_id,
            recovery_blob,
            backup_id,
            chunk_size,
        })
    }
}

fn bad_file(what: &str) -> ApiError {
    ApiError::new(
        kb_types::ErrorCode::Crypto,
        format!("not a valid backup file: {what}"),
    )
}

fn array16(bytes: &[u8]) -> Result<[u8; 16], ApiError> {
    bytes.try_into().map_err(|_| bad_file("16-byte field"))
}

fn array8(bytes: &[u8]) -> Result<[u8; 8], ApiError> {
    bytes.try_into().map_err(|_| bad_file("8-byte field"))
}

/// `HKDF(ARK, backup, info = backup id)`.
fn backup_key(ark: &Ark, backup_id: &[u8; 16]) -> Secret32 {
    kb_crypto::derive_key(ark.secret(), Label::BACKUP, backup_id)
}

/// Buffers plaintext records and seals full chunks into frames on the sink.
struct FrameWriter<'a> {
    sealer: StreamSealer,
    chunk_size: usize,
    buffer: Vec<u8>,
    sink: &'a mut dyn FnMut(&[u8]) -> Result<(), String>,
    bytes_out: u64,
}

impl<'a> FrameWriter<'a> {
    fn write_record(
        &mut self,
        tag: u8,
        body: &[u8],
        rng: &dyn kb_types::Rng,
    ) -> Result<(), ApiError> {
        let len = u32::try_from(body.len()).map_err(|_| ApiError::internal("record too large"))?;
        self.buffer.push(tag);
        self.buffer.extend_from_slice(&len.to_le_bytes());
        self.buffer.extend_from_slice(body);
        while self.buffer.len() >= self.chunk_size {
            let rest = self.buffer.split_off(self.chunk_size);
            let chunk = std::mem::replace(&mut self.buffer, rest);
            self.emit(&chunk, false, rng)?;
        }
        Ok(())
    }

    fn finish(mut self, rng: &dyn kb_types::Rng) -> Result<u64, ApiError> {
        let chunk = std::mem::take(&mut self.buffer);
        self.emit(&chunk, true, rng)?;
        Ok(self.bytes_out)
    }

    fn emit(
        &mut self,
        plaintext: &[u8],
        is_final: bool,
        rng: &dyn kb_types::Rng,
    ) -> Result<(), ApiError> {
        let sealed = self
            .sealer
            .seal_chunk(plaintext, is_final, rng)
            .map_err(|e| ApiError::crypto(format!("backup sealing failed: {e}")))?;
        let len = u32::try_from(sealed.len()).map_err(|_| ApiError::internal("chunk too large"))?;
        let mut frame = Vec::with_capacity(5 + sealed.len());
        frame.push(u8::from(is_final));
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(&sealed);
        (self.sink)(&frame)
            .map_err(|e| ApiError::storage(format!("cannot write the backup: {e}")))?;
        self.bytes_out += frame.len() as u64;
        Ok(())
    }
}

/// Reads exactly `buf.len()` bytes from the source callback.
fn read_exact(
    source: &mut dyn FnMut(&mut [u8]) -> Result<usize, String>,
    buf: &mut [u8],
) -> Result<bool, ApiError> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = source(&mut buf[filled..])
            .map_err(|e| ApiError::storage(format!("cannot read the backup: {e}")))?;
        if n == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(bad_file("truncated"));
        }
        filled += n;
    }
    Ok(true)
}

/// A decoded document record.
struct DocRecord {
    row: DocRow,
    snapshot: Vec<u8>,
}

/// Everything a backup holds, decoded and validated before anything is written. Built
/// record by record; `finish` checks that the mandatory records were there.
#[derive(Default)]
struct Restored {
    workspace: Option<WorkspaceRow>,
    settings: Vec<SettingRow>,
    log: Vec<LogEntry>,
    keyring: Vec<KeyringItem>,
    docs: Vec<DocRecord>,
}

/// The decoded records once the end record was seen.
struct Validated {
    workspace: WorkspaceRow,
    settings: Vec<SettingRow>,
    log: Vec<LogEntry>,
    keyring: Vec<KeyringItem>,
    docs: Vec<DocRecord>,
}

impl Restored {
    fn finish(self) -> Result<Validated, ApiError> {
        let workspace = self
            .workspace
            .ok_or_else(|| bad_file("no workspace record"))?;
        if self.log.is_empty() {
            return Err(bad_file("no device log"));
        }
        Ok(Validated {
            workspace,
            settings: self.settings,
            log: self.log,
            keyring: self.keyring,
            docs: self.docs,
        })
    }
}

/// Pulls frames from the source, decrypts them and hands out whole records, holding at
/// most one frame plus the record being assembled.
struct FrameSource<'a> {
    source: &'a mut dyn FnMut(&mut [u8]) -> Result<usize, String>,
    opener: StreamOpener,
    max_frame: usize,
    buffer: Vec<u8>,
    pos: usize,
    saw_final: bool,
    bytes_in: u64,
}

impl FrameSource<'_> {
    /// Reads and decrypts the next frame into the buffer. `Ok(false)` once the final
    /// frame has been consumed.
    fn fill(&mut self) -> Result<bool, ApiError> {
        if self.saw_final {
            return Ok(false);
        }
        let mut frame_header = [0u8; 5];
        if !read_exact(self.source, &mut frame_header)? {
            return Err(bad_file("truncated before the final chunk"));
        }
        let is_final = match frame_header[0] {
            0 => false,
            1 => true,
            _ => return Err(bad_file("frame flag")),
        };
        let len = u32::from_le_bytes([
            frame_header[1],
            frame_header[2],
            frame_header[3],
            frame_header[4],
        ]) as usize;
        if len > self.max_frame {
            return Err(bad_file("chunk length"));
        }
        let mut sealed = vec![0u8; len];
        read_exact(self.source, &mut sealed)?;
        self.bytes_in += (5 + len) as u64;
        let chunk = self
            .opener
            .open_chunk(&sealed, is_final)
            .map_err(|_| ApiError::crypto("the backup is damaged or the key is wrong"))?;
        // Drop what was consumed before appending, so the buffer never grows past one
        // frame plus the record in progress.
        if self.pos > 0 {
            self.buffer.drain(..self.pos);
            self.pos = 0;
        }
        self.buffer.extend_from_slice(&chunk);
        self.saw_final = is_final;
        Ok(true)
    }

    /// Makes at least `n` unread bytes available, or reports the end of the stream.
    fn ensure(&mut self, n: usize) -> Result<bool, ApiError> {
        while self.buffer.len() - self.pos < n {
            if !self.fill()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The next `tag, body` record; `None` at the end of the stream.
    fn next_record(&mut self) -> Result<Option<(u8, Vec<u8>)>, ApiError> {
        if !self.ensure(5)? {
            return Ok(if self.buffer.len() == self.pos {
                None
            } else {
                return Err(bad_file("truncated record"));
            });
        }
        let tag = self.buffer[self.pos];
        let len = u32::from_le_bytes([
            self.buffer[self.pos + 1],
            self.buffer[self.pos + 2],
            self.buffer[self.pos + 3],
            self.buffer[self.pos + 4],
        ]);
        if len > MAX_RECORD_LEN {
            return Err(bad_file("record too large"));
        }
        let len = len as usize;
        if !self.ensure(5 + len)? {
            return Err(bad_file("truncated record"));
        }
        let start = self.pos + 5;
        let body = self.buffer[start..start + len].to_vec();
        self.pos = start + len;
        Ok(Some((tag, body)))
    }

    /// The stream must have ended with its final frame, fully authenticated.
    fn finish(&mut self) -> Result<(), ApiError> {
        if !self.saw_final {
            // Records ended but frames go on: read to the final frame so that the
            // authentication covers the whole file.
            while self.fill()? {}
        }
        self.opener
            .finish()
            .map_err(|_| ApiError::crypto("the backup is damaged or the key is wrong"))
    }
}

impl Core {
    /// Streams an encrypted backup of the whole workspace to `sink` (ADR-0017).
    pub fn backup_export(
        &mut self,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<BackupSummary, ApiError> {
        let workspace = self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::invalid("workspace", "there is no workspace to back up"))?;
        let recovery = self.security.recovery.as_ref().ok_or_else(|| {
            ApiError::invalid("kit", "create your recovery kit before exporting a backup")
        })?;
        let account_id = self
            .security
            .log_state
            .as_ref()
            .map(|s| s.account_id)
            .ok_or_else(|| {
                ApiError::invalid("kit", "create your recovery kit before exporting a backup")
            })?;
        if self.security.recovery_stale {
            return Err(ApiError::invalid(
                "kit",
                "re-issue your recovery kit before exporting a backup",
            ));
        }
        let now = self.clock.now_ms();
        let mut backup_id = [0u8; 16];
        self.rng.fill_bytes(&mut backup_id);
        let header = Header {
            account_id,
            created_at: now,
            recovery_id: recovery.id,
            recovery_blob: recovery.blob.clone(),
            backup_id,
            chunk_size: u32::try_from(CHUNK_SIZE).unwrap_or(u32::MAX),
        };
        let header_bytes = header.encode();
        let mut prefix = Vec::with_capacity(4 + 2 + 1 + 4 + header_bytes.len());
        prefix.extend_from_slice(BACKUP_MAGIC);
        prefix.extend_from_slice(&BACKUP_FORMAT.to_le_bytes());
        prefix.push(SUITE);
        prefix.extend_from_slice(&u32::try_from(header_bytes.len()).unwrap_or(0).to_le_bytes());
        prefix.extend_from_slice(&header_bytes);
        sink(&prefix).map_err(|e| ApiError::storage(format!("cannot write the backup: {e}")))?;

        let key = backup_key(&self.ark, &backup_id);
        let mut writer = FrameWriter {
            sealer: StreamSealer::new(key, kb_crypto::hash(&header_bytes).to_vec()),
            chunk_size: CHUNK_SIZE,
            buffer: Vec::new(),
            sink,
            bytes_out: prefix.len() as u64,
        };
        let rng = self.rng.as_ref();

        writer.write_record(TAG_WORKSPACE, &encode_workspace(&workspace), rng)?;
        let mut settings = self.db.settings_all().map_err(map_storage_error)?;
        settings.retain(|setting| !DEVICE_BOUND_SETTINGS.contains(&setting.key.as_str()));
        for setting in &settings {
            writer.write_record(TAG_SETTING, &encode_setting(setting)?, rng)?;
        }
        for row in self.db.device_log_all().map_err(map_storage_error)? {
            writer.write_record(TAG_LOG_ENTRY, &row.entry, rng)?;
        }
        for row in self.db.keyring_all().map_err(map_storage_error)? {
            writer.write_record(TAG_KEYRING, &row.record, rng)?;
        }
        let rows = self.db.docs_all().map_err(map_storage_error)?;
        let mut documents = 0u32;
        for row in &rows {
            let snapshot = self.snapshot_of(row)?;
            writer.write_record(TAG_DOCUMENT, &encode_document(row, &snapshot), rng)?;
            documents += 1;
        }
        // Attachment chunks (ADR-0020), one record per chunk, after the documents.
        let mut chunk_ids: Vec<ChunkId> = Vec::new();
        for open in self.docs.structures.values() {
            for manifest in open.model.attachments() {
                chunk_ids.extend(manifest.chunks);
            }
        }
        let mut chunks = 0u32;
        let mut chunk_bytes = 0u64;
        let mut missing = 0u32;
        for id in &chunk_ids {
            match self.blob_store.get(id).map_err(map_storage_error)? {
                Some(sealed) => {
                    let mut body = Vec::with_capacity(32 + sealed.len());
                    body.extend_from_slice(id.as_bytes());
                    body.extend_from_slice(&sealed);
                    writer.write_record(TAG_CHUNK, &body, rng)?;
                    chunks += 1;
                    chunk_bytes += sealed.len() as u64;
                }
                None => missing += 1,
            }
        }
        if missing > 0 {
            tracing::warn!(
                missing,
                "attachment chunks missing from the store were left out of the backup"
            );
        }
        writer.write_record(TAG_END, &[], rng)?;
        let bytes = writer.finish(rng)?;
        tracing::info!(documents, chunks, bytes, "backup exported");
        Ok(BackupSummary {
            bytes,
            documents,
            settings: u32::try_from(settings.len()).unwrap_or(u32::MAX),
            created_at: now,
            account_id,
            chunks,
            chunk_bytes,
        })
    }

    /// A fresh snapshot of a document: from the open model when loaded, otherwise
    /// loaded transiently from storage.
    fn snapshot_of(&self, row: &DocRow) -> Result<Vec<u8>, ApiError> {
        let doc = match row.kind {
            DocKindCode::Account => self
                .docs
                .account
                .as_ref()
                .map(|a| a.model.doc().export_snapshot()),
            DocKindCode::Structure => row
                .space_id
                .and_then(|s| self.docs.structures.get(&s))
                .map(|o| o.model.doc().export_snapshot()),
            DocKindCode::Page => {
                let page_id = kb_types::PageId::from_bytes(row.doc_id.to_bytes());
                self.docs
                    .pages
                    .get(&page_id)
                    .map(|p| p.model.doc().export_snapshot())
            }
            // A comment stream's backup content is its records snapshot (the records the fold
            // counts), not a Loro snapshot (ADR-0048: backups include comments).
            DocKindCode::Comments => {
                return self.comment_stream_snapshot_of_doc(row);
            }
        };
        if let Some(result) = doc {
            return result.map_err(map_doc_error);
        }
        let kind = match row.kind {
            DocKindCode::Account => kb_doc::DocKind::Account,
            DocKindCode::Structure => kb_doc::DocKind::Structure,
            DocKindCode::Page => kb_doc::DocKind::Page,
            DocKindCode::Comments => kb_doc::DocKind::Comments,
        };
        let doc = crate::documents::load_doc(&self.db, row, kind, self.rng.as_ref())?;
        doc.export_snapshot().map_err(map_doc_error)
    }

    /// Restores a backup read from `source` into this (empty) installation, with the
    /// recovery kit text when the backup belongs to another installation's account.
    pub fn backup_import(
        &mut self,
        source: &mut dyn FnMut(&mut [u8]) -> Result<usize, String>,
        kit: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some()
        {
            return Err(ApiError::already_exists(
                "this installation already has a workspace; restoring needs a fresh one",
            ));
        }

        // Prefix and header.
        let mut fixed = [0u8; 4 + 2 + 1 + 4];
        if !read_exact(source, &mut fixed)? {
            return Err(bad_file("empty file"));
        }
        if &fixed[..4] != BACKUP_MAGIC {
            return Err(bad_file("bad magic"));
        }
        let format = u16::from_le_bytes([fixed[4], fixed[5]]);
        if format == 0 || format > BACKUP_FORMAT {
            return Err(ApiError::new(
                kb_types::ErrorCode::Storage,
                format!("this backup needs a newer app version (format {format})"),
            ));
        }
        if fixed[6] != SUITE {
            return Err(bad_file("unknown cryptographic suite"));
        }
        let header_len = u32::from_le_bytes([fixed[7], fixed[8], fixed[9], fixed[10]]) as usize;
        if header_len != HEADER_LEN {
            return Err(bad_file("header length"));
        }
        let mut header_bytes = vec![0u8; header_len];
        read_exact(source, &mut header_bytes)?;
        let header = Header::decode(&header_bytes)?;
        if header.chunk_size == 0 || header.chunk_size as usize > 64 * 1024 * 1024 {
            return Err(bad_file("chunk size"));
        }

        // The root key: from the kit, or this installation's own when the account matches.
        let (ark, via_kit) = match kit {
            Some(kit) => {
                let rck = RecoveryKey::from_kit(kit.trim()).map_err(map_keys_error)?;
                if rck.recovery_id() != header.recovery_id {
                    return Err(ApiError::invalid(
                        "kit",
                        "this kit does not belong to this backup",
                    ));
                }
                let ark = rck
                    .unwrap_ark(&header.recovery_blob)
                    .map_err(|_| ApiError::invalid("kit", "this kit does not open this backup"))?;
                (ark, Some(rck))
            }
            None => {
                let own = self.security.log_state.as_ref().map(|s| s.account_id);
                if own != Some(header.account_id) {
                    return Err(ApiError::invalid(
                        "kit",
                        "the recovery kit is required to restore this backup",
                    ));
                }
                (self.ark.clone(), None)
            }
        };

        // Decrypt frame by frame and decode one record at a time: metadata is held until
        // the end record, chunks go to the blob store as they arrive. Nothing touches the
        // database before every record has been read and verified.
        let key = backup_key(&ark, &header.backup_id);
        let mut frames = FrameSource {
            source,
            opener: StreamOpener::new(key, kb_crypto::hash(&header_bytes).to_vec()),
            max_frame: header.chunk_size as usize + 64,
            buffer: Vec::new(),
            pos: 0,
            saw_final: false,
            bytes_in: (fixed.len() + header_bytes.len()) as u64,
        };
        let mut builder = Restored::default();
        let mut staged: Vec<ChunkId> = Vec::new();
        let mut chunks = 0u32;
        let mut chunk_bytes = 0u64;
        let outcome = loop {
            let (tag, body) = match frames.next_record() {
                Ok(Some(record)) => record,
                Ok(None) => break Err(bad_file("no end record")),
                Err(err) => break Err(err),
            };
            if tag == TAG_END {
                break Ok(());
            }
            if tag == TAG_CHUNK {
                if format < FORMAT_WITH_CHUNKS {
                    break Err(bad_file("chunk record in a format 1 file"));
                }
                if body.len() < 32 || body.len() > 32 + CHUNK_SIZE + CHUNK_OVERHEAD + 64 {
                    break Err(bad_file("chunk record length"));
                }
                let (id_bytes, sealed) = body.split_at(32);
                let id = match array32(id_bytes) {
                    Ok(id) => ChunkId::from_bytes(id),
                    Err(err) => break Err(err),
                };
                if ChunkId::from_bytes(kb_crypto::hash(sealed)) != id {
                    break Err(bad_file("chunk hash"));
                }
                if let Err(err) = self.blob_store.put(&id, sealed).map_err(map_storage_error) {
                    break Err(err);
                }
                staged.push(id);
                chunks += 1;
                chunk_bytes += sealed.len() as u64;
                continue;
            }
            if let Err(err) = parse_metadata_record(tag, &body, &mut builder) {
                break Err(err);
            }
        };
        let outcome = outcome.and_then(|()| frames.finish());
        let mut restored = match outcome.and_then(|()| builder.finish()) {
            Ok(restored) => restored,
            Err(err) => {
                // A failed restore leaves no chunks behind (the sweep would catch them
                // anyway, but a half-written restore should not look like a cache).
                for id in &staged {
                    let _ = self.blob_store.delete(id);
                }
                return Err(err);
            }
        };
        let bytes_in = frames.bytes_in;

        // Validate the device log and the keyring items before writing.
        let log = DeviceLog::from_entries(restored.log.clone())
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;
        let state = log
            .verify()
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;
        if state.account_id != header.account_id {
            return Err(bad_file(
                "the device log does not belong to the header's account",
            ));
        }
        // The key records of this account's own spaces are signed by its own devices and are checked against the log the file
        // carries. The records of a shared space are signed by the other members' devices too, whose logs the file does not carry:
        // they are not vouched for here, they are left out, and the new device gets them from the shared feed, where each is
        // checked against its member's own device log. A signer unknown to the file in any other space is a bad file.
        let shared = self.shared_spaces_in(&restored.docs)?;
        let mut kept = Vec::with_capacity(restored.keyring.len());
        let mut left_out = 0u32;
        for item in std::mem::take(&mut restored.keyring) {
            let signer = match &item {
                KeyringItem::Record(r) => r.created_by,
                KeyringItem::Tombstone(t) => t.purged_by,
            };
            match state.devices.iter().find(|d| d.device_id == signer) {
                Some(device) => {
                    item.verify(&device.keys.signing).map_err(map_keys_error)?;
                    kept.push(item);
                }
                None if shared.contains(&item.space_id()) => left_out += 1,
                None => {
                    return Err(ApiError::crypto(
                        "a keyring item is signed by an unknown device",
                    ));
                }
            }
        }
        if left_out > 0 {
            tracing::debug!(
                left_out,
                "key records of other members were left out of the restore: the shared feeds bring them back"
            );
        }
        restored.keyring = kept;

        // Write everything in one transaction, then switch this installation to the
        // restored root key.
        let settings_count = u32::try_from(restored.settings.len()).unwrap_or(u32::MAX);
        let documents = u32::try_from(restored.docs.len()).unwrap_or(u32::MAX);
        let now = self.clock.now_ms();
        let recovery_hex = hex(&header.recovery_id);
        let blob_hex = hex(&header.recovery_blob.to_bytes());
        let account_text = header.account_id.to_string();
        let created_text = header.created_at.to_string();
        self.db
            .transaction(|tx| {
                if !tx.workspace_insert(&restored.workspace)? {
                    return Err(kb_storage::StorageError::InvalidData(
                        "workspace exists".into(),
                    ));
                }
                for setting in &restored.settings {
                    // A crafted backup must not carry another device's consent or folder.
                    if DEVICE_BOUND_SETTINGS.contains(&setting.key.as_str()) {
                        continue;
                    }
                    tx.setting_put(&setting.key, &setting.value, setting.updated_at)?;
                }
                for entry in &restored.log {
                    tx.device_log_append(&DeviceLogRow {
                        seq: entry.seq,
                        entry: entry.encode(),
                        hash: entry.hash(),
                    })?;
                }
                for item in &restored.keyring {
                    tx.keyring_put(&keyring_row(item))?;
                }
                for doc in &restored.docs {
                    tx.doc_insert(&doc.row)?;
                    // A comment stream has no snapshot row: its records are stored in `comments`
                    // once the pages are known (`comments_restore` below).
                    if doc.row.kind == DocKindCode::Comments {
                        continue;
                    }
                    tx.snapshot_put(
                        doc.row.doc_id,
                        &version_of(&doc.snapshot)?,
                        &doc.snapshot,
                        now,
                    )?;
                }
                tx.meta_set(META_ACCOUNT_ID, &account_text)?;
                tx.meta_set(META_RECOVERY_ID, &recovery_hex)?;
                tx.meta_set(META_RECOVERY_BLOB, &blob_hex)?;
                tx.meta_set(META_RECOVERY_CREATED, &created_text)?;
                Ok(())
            })
            .map_err(map_storage_error)?;

        if via_kit.is_some() {
            self.park_and_switch(ark)?;
        }
        self.security.lock = None;

        // Reload everything from the restored rows.
        self.docs = crate::documents::load_documents(&self.db, self.rng.as_ref())?;
        crate::documents::verify_index(&mut self.db, &self.docs)?;
        // The comments of the backup come back (the pages they are about are known now).
        for doc in &restored.docs {
            if doc.row.kind == DocKindCode::Comments {
                self.comments_restore(&doc.row, &doc.snapshot)?;
            }
        }
        self.security = Core::load_security(
            &self.db,
            &self.ark,
            self.security.device_keys.clone(),
            self.device.device_id,
            None,
        )?;
        self.ensure_account()?;
        self.verify_text_index()?;
        self.index_rebuild(None)?;
        // The attachment index is derived: build it from the restored manifests.
        self.verify_attachment_index()?;

        // This device joins the account through the kit.
        if let Some(rck) = via_kit
            && self
                .security
                .log_state
                .as_ref()
                .is_some_and(|s| s.active(self.device.device_id).is_none())
        {
            let signing = rck.signing_key();
            let name = kb_keys::log::encrypt_name(
                &self.ark,
                self.device.device_id,
                &self.device.name,
                self.rng.as_ref(),
            );
            let entry = self
                .security
                .log
                .as_mut()
                .ok_or_else(|| ApiError::internal("restored log missing"))?
                .add_device(
                    self.device.device_id,
                    self.security.device_keys.public(),
                    name,
                    LogSigner::Recovery(&signing),
                    now,
                )
                .map_err(|e| map_keys_error(KeysError::Log(e)))?
                .clone();
            self.db
                .device_log_append(&DeviceLogRow {
                    seq: entry.seq,
                    entry: entry.encode(),
                    hash: entry.hash(),
                })
                .map_err(map_storage_error)?;
            if let Some(log) = &self.security.log {
                self.security.log_state = Some(
                    log.verify()
                        .map_err(|e| map_keys_error(KeysError::Log(e)))?,
                );
            }
            if let Some(account) = self.docs.account.as_ref() {
                let _ = account
                    .model
                    .set_device_name(self.device.device_id, &self.device.name);
                self.persist_account()?;
            }
        }
        self.keyring_maintain()?;

        self.events.push(Event::WorkspaceCreated {
            id: restored.workspace.id,
        });
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::SecurityChanged);
        tracing::info!(documents, chunks, bytes = bytes_in, "backup restored");
        Ok(BackupSummary {
            bytes: bytes_in,
            documents,
            settings: settings_count,
            created_at: header.created_at,
            account_id: header.account_id,
            chunks,
            chunk_bytes,
        })
    }

    /// Switches this installation to `ark` with the same two-phase protocol as a
    /// rotation (park, rekey, promote).
    /// The spaces the account document of a backup lists as shared.
    fn shared_spaces_in(&self, docs: &[DocRecord]) -> Result<HashSet<SpaceId>, ApiError> {
        let mut shared = HashSet::new();
        for record in docs.iter().filter(|d| d.row.kind == DocKindCode::Account) {
            let doc = Doc::load(
                DocKind::Account,
                crate::documents::new_peer(self.rng.as_ref()),
                Some(&record.snapshot),
                &[],
            )
            .map_err(map_doc_error)?;
            let model = AccountDoc::from_doc(doc).map_err(crate::documents::map_model_error)?;
            shared.extend(model.shared_spaces().into_iter().map(|(space, _)| space));
        }
        Ok(shared)
    }

    pub(crate) fn park_and_switch(&mut self, ark: Ark) -> Result<(), ApiError> {
        self.keystore
            .set(
                &format!("ark-next:{}", self.install_id),
                ark.secret().expose(),
            )
            .map_err(|e| map_keys_error(KeysError::Keystore(e)))?;
        let key = kb_keys::derive_db_key(&ark, &self.install_id);
        self.db.rekey(&key).map_err(map_storage_error)?;
        kb_keys::lock::store_ark(
            self.keystore.as_ref(),
            &self.install_id,
            &ark,
            None,
            self.rng.as_ref(),
        )
        .map_err(map_keys_error)?;
        let _ = self
            .keystore
            .delete(&format!("ark-next:{}", self.install_id));
        let _ = self
            .keystore
            .delete(&format!("ark-next-locked:{}", self.install_id));
        self.ark = ark;
        Ok(())
    }
}

/// The version of an enveloped snapshot (loaded into a scratch document).
fn version_of(snapshot: &[u8]) -> Result<Vec<u8>, kb_storage::StorageError> {
    let doc = kb_doc::Doc::load(kb_doc::DocKind::Page, 1, Some(snapshot), &[])
        .map_err(|e| kb_storage::StorageError::InvalidData(format!("snapshot: {e}")))?;
    Ok(doc.version().0)
}

fn encode_workspace(row: &WorkspaceRow) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(row.id.as_bytes());
    out.extend_from_slice(&row.created_at.to_le_bytes());
    push_bytes(&mut out, row.name.as_bytes());
    out
}

fn encode_setting(row: &SettingRow) -> Result<Vec<u8>, ApiError> {
    let value =
        serde_json::to_vec(&row.value).map_err(|e| ApiError::internal(format!("setting: {e}")))?;
    let mut out = Vec::new();
    out.extend_from_slice(&row.updated_at.to_le_bytes());
    push_bytes(&mut out, row.key.as_bytes());
    push_bytes(&mut out, &value);
    Ok(out)
}

fn encode_document(row: &DocRow, snapshot: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(row.doc_id.as_bytes());
    out.push(match row.kind {
        DocKindCode::Structure => 0,
        DocKindCode::Page => 1,
        DocKindCode::Account => 2,
        DocKindCode::Comments => 3,
    });
    match row.space_id {
        Some(space) => {
            out.push(1);
            out.extend_from_slice(space.as_bytes());
        }
        None => out.push(0),
    }
    out.extend_from_slice(&row.created_at.to_le_bytes());
    out.extend_from_slice(&row.format_version.to_le_bytes());
    out.extend_from_slice(&row.min_reader_version.to_le_bytes());
    push_bytes(&mut out, snapshot);
    out
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// A cursor over plaintext records.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ApiError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| bad_file("record length"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| bad_file("truncated record"))?;
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, ApiError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ApiError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, ApiError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Result<i64, ApiError> {
        Ok(i64::from_le_bytes(array8(self.take(8)?)?))
    }
    fn bytes(&mut self) -> Result<&'a [u8], ApiError> {
        let len = self.u32()?;
        if len > MAX_RECORD_LEN {
            return Err(bad_file("record too large"));
        }
        self.take(len as usize)
    }
    fn id16(&mut self) -> Result<[u8; 16], ApiError> {
        array16(self.take(16)?)
    }
}

/// Decodes one metadata record into the builder (chunk and end records are handled by
/// the caller).
fn parse_metadata_record(tag: u8, body: &[u8], out: &mut Restored) -> Result<(), ApiError> {
    let mut r = Reader {
        bytes: body,
        pos: 0,
    };
    match tag {
        TAG_WORKSPACE => {
            let id = WorkspaceId::from_bytes(r.id16()?);
            let created_at = r.i64()?;
            let name =
                String::from_utf8(r.bytes()?.to_vec()).map_err(|_| bad_file("workspace name"))?;
            out.workspace = Some(WorkspaceRow {
                id,
                name,
                created_at,
            });
        }
        TAG_SETTING => {
            let updated_at = r.i64()?;
            let key =
                String::from_utf8(r.bytes()?.to_vec()).map_err(|_| bad_file("setting key"))?;
            let value: Json =
                serde_json::from_slice(r.bytes()?).map_err(|_| bad_file("setting value"))?;
            out.settings.push(SettingRow {
                key,
                value,
                updated_at,
            });
        }
        TAG_LOG_ENTRY => {
            out.log
                .push(LogEntry::decode(body).map_err(|e| map_keys_error(KeysError::Log(e)))?);
        }
        TAG_KEYRING => {
            out.keyring
                .push(KeyringItem::decode(body).map_err(map_keys_error)?);
        }
        TAG_DOCUMENT => {
            let doc_id = DocId::from_bytes(r.id16()?);
            let kind = match r.u8()? {
                0 => DocKindCode::Structure,
                1 => DocKindCode::Page,
                2 => DocKindCode::Account,
                3 => DocKindCode::Comments,
                _ => return Err(bad_file("document kind")),
            };
            let space_id = match r.u8()? {
                0 => None,
                1 => Some(SpaceId::from_bytes(r.id16()?)),
                _ => return Err(bad_file("document space")),
            };
            let created_at = r.i64()?;
            let format_version = r.u16()?;
            let min_reader_version = r.u16()?;
            let snapshot = r.bytes()?.to_vec();
            if min_reader_version > kb_doc::FORMAT_VERSION {
                return Err(ApiError::new(
                    kb_types::ErrorCode::Storage,
                    "this backup holds documents that need a newer app version",
                ));
            }
            out.docs.push(DocRecord {
                row: DocRow {
                    doc_id,
                    kind,
                    space_id,
                    created_at,
                    format_version,
                    min_reader_version,
                },
                snapshot,
            });
        }
        _ => return Err(bad_file("unknown record")),
    }
    Ok(())
}

fn array32(bytes: &[u8]) -> Result<[u8; 32], ApiError> {
    bytes
        .try_into()
        .map_err(|_| bad_file("a 32-byte field has the wrong length"))
}
