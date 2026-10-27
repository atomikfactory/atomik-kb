//! Sync (Milestone 6, ADR-0021; protocol v2 since Milestone 13, ADR-0045): the sans-IO half of
//! what a sync cycle does beyond one feed. The driver in `kb-net` (or the fleet simulator of the
//! tests) calls these methods; the core never touches the network.
//!
//! This file holds the configuration and the status (`SyncConfigure`, `sync_info`, the server
//! epoch), the identity and the device log (`sync_identity`, `sync_account_log`,
//! `sync_log_received`), the keys a document is read and sealed under, and what applying an item of
//! a feed does to the open documents (`apply_account_updates`, `apply_structure_updates`,
//! `apply_page_updates`). The per-feed steps (outbox, apply path, snapshots, heads, chunks) are in
//! `feeds.rs` and its submodules, the membership log in `membership.rs`.
//!
//! Applying: an item is verified against the device log, checked for sequence continuity, decrypted
//! under the document key (or the keyring key), imported into the open model (page documents are
//! loaded transiently when no editor has them), persisted as remote updates and re-indexed; the
//! cursor and chain hash of its feed advance after each item. Entries that cannot be applied yet (key
//! or structure missing) wait in `sync_pending` and are retried when something new was applied.

use std::collections::{BTreeSet, HashMap, HashSet};

use kb_crypto::{Label, Secret32, Signature, SigningKey, VerifyingKey};
use kb_doc::{Doc, DocKind, Frontiers};
use kb_keys::keyring as kr;
use kb_keys::{KeySubject, KeyringItem, KeysError, LogEntry};
use kb_model::{PageDoc, StructureDoc};
use kb_storage::{
    DeviceLogRow, DocKindCode, DocQueries, DocRow, FeedQueries, JournalKind, PersonMentionQueries,
    SecurityQueries, StorageError, SyncDocState, SyncQueries, UpdateOrigin,
};
use kb_sync::SyncError;
use kb_types::sync::{KEYRING_DOC_ID, SNAPSHOT_AFTER_ENTRIES};
use kb_types::{
    AccountId, ApiError, ChangeSource, DeviceId, DocId, Event, PageId, Request, Response, SpaceId,
    SyncInfo, SyncState,
};

use crate::documents::{self, OpenPage, OpenStructure, map_doc_error, map_model_error};
use crate::history::{JournalWrite, Writer};
use crate::security::{decode_hex, hex};
use crate::{Core, map_keys_error, map_storage_error, short_id};

// ----- configuration keys (sync_config) ---------------------------------------------------
const CFG_SERVER_URL: &str = "server_url";
const CFG_SERVER_EPOCH: &str = "server_epoch";
const CFG_LAST_PUSH: &str = "last_push_at";
const CFG_LAST_PULL: &str = "last_pull_at";
const CFG_LAST_ERROR: &str = "last_error";
const CFG_NEEDS_INVITE: &str = "needs_invite_token";
/// The single-feed keys of protocol v1 (cursor, chain hash, fork flag, highest snapshot cover, chain
/// history). Nothing writes them any more (`sync_feeds` keeps them per feed); an installation that
/// still has them loses them at its next server change.
const LEGACY_V1_KEYS: [&str; 5] = [
    "remote_cursor",
    "chain_hash",
    "fork_detected",
    "max_covers",
    "chain_history",
];

/// Reasons an item waits in `sync_pending`.
pub(crate) const PENDING_KEY: u8 = 1;
pub(crate) const PENDING_STRUCTURE: u8 = 2;
pub(crate) const PENDING_SNAPSHOT: u8 = 3;
/// An entry whose changes depend on history this device does not have (the snapshot
/// that brings it is still to come).
pub(crate) const PENDING_HISTORY: u8 = 4;

/// The message shown while the server does not know the account.
pub const ACCOUNT_UNKNOWN_MESSAGE: &str =
    "the server does not know this account; an invite token is required";

/// In-memory sync state (the durable part lives in `sync_config`).
pub(crate) struct SyncRuntime {
    pub(crate) state: SyncState,
    pub(crate) last_error: Option<String>,
    /// Kept only until the driver takes it; never persisted.
    pub(crate) invite_token: Option<String>,
    /// Spaces the keyring holds records for, cached for one batch of items.
    pub(crate) known_spaces: Option<Vec<SpaceId>>,
}

impl Default for SyncRuntime {
    fn default() -> Self {
        Self {
            state: SyncState::Disabled,
            last_error: None,
            invite_token: None,
            known_spaces: None,
        }
    }
}

pub(crate) fn map_sync_error(err: SyncError) -> ApiError {
    match err {
        SyncError::Authentication => ApiError::crypto("sync data failed authentication"),
        SyncError::Encoding(msg) => ApiError::storage(format!("malformed sync data: {msg}")),
        other => ApiError::storage(format!("sync protocol: {other}")),
    }
}

pub(crate) fn doc_kind_code(kind: u8) -> Option<DocKindCode> {
    match kind {
        0 => Some(DocKindCode::Structure),
        1 => Some(DocKindCode::Page),
        2 => Some(DocKindCode::Account),
        3 => Some(DocKindCode::Comments),
        _ => None,
    }
}

pub(crate) fn code_of(kind: DocKindCode) -> u8 {
    match kind {
        DocKindCode::Structure => 0,
        DocKindCode::Page => 1,
        DocKindCode::Account => 2,
        DocKindCode::Comments => 3,
    }
}

/// The engine of a document kind's blobs (`kb_doc::ENGINE_LORO`, or the records engine of a comment
/// stream).
pub(crate) fn engine_of(kind: DocKindCode) -> u8 {
    match kind {
        DocKindCode::Comments => kb_doc::ENGINE_RECORDS,
        _ => kb_doc::ENGINE_LORO,
    }
}

pub(crate) fn engine_kind(kind: DocKindCode) -> DocKind {
    match kind {
        DocKindCode::Structure => DocKind::Structure,
        DocKindCode::Page => DocKind::Page,
        DocKindCode::Account => DocKind::Account,
        DocKindCode::Comments => DocKind::Comments,
    }
}

impl Core {
    // ----- requests -------------------------------------------------------------------------

    /// Routes the sync requests.
    pub(crate) fn dispatch_sync(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::SyncConfigure {
                server_url,
                invite_token,
            } => {
                self.sync_configure(server_url.as_deref(), invite_token)?;
                Ok(Response::Sync(self.sync_info()?))
            }
            Request::SyncStatus => Ok(Response::Sync(self.sync_info()?)),
            Request::SyncNow => {
                if self.sync_server_url()?.is_some() {
                    self.events.push(Event::SyncRequested);
                }
                Ok(Response::Ok)
            }
            other => Err(ApiError::internal(format!(
                "{} is not a sync request",
                other.kind()
            ))),
        }
    }

    /// Points this device at a server, or disables sync. Another server than before
    /// means another feed: the remote state is forgotten and everything becomes
    /// unpublished again.
    pub(crate) fn sync_configure(
        &mut self,
        server_url: Option<&str>,
        invite_token: Option<String>,
    ) -> Result<(), ApiError> {
        match server_url {
            Some(url) => {
                let url = url.trim().trim_end_matches('/');
                if !(url.starts_with("http://") || url.starts_with("https://"))
                    || url.len() > 2048
                    || url.contains(['@', '?', '#', ' '])
                {
                    return Err(ApiError::invalid(
                        "serverUrl",
                        "the server address must be http(s)://host[:port]",
                    ));
                }
                if self.sync_identity()?.is_none() {
                    return Err(ApiError::invalid(
                        "serverUrl",
                        "create your recovery kit before turning sync on",
                    ));
                }
                let token = match invite_token {
                    Some(token) => {
                        let token = token.trim().to_owned();
                        if token.is_empty()
                            || token.len() > 128
                            || !token.bytes().all(|b| (0x20..=0x7e).contains(&b))
                        {
                            return Err(ApiError::invalid(
                                "inviteToken",
                                "the invite token must be 1 to 128 printable ASCII characters",
                            ));
                        }
                        Some(token)
                    }
                    None => None,
                };
                let changed = self.sync_server_url()?.as_deref() != Some(url);
                self.db
                    .sync_config_set(CFG_SERVER_URL, url)
                    .map_err(map_storage_error)?;
                if changed {
                    self.sync_forget_remote_state()?;
                    self.chunk_sync_on_server_change()?;
                }
                self.sync.invite_token = token;
                self.sync.state = SyncState::Idle;
                self.sync.last_error = None;
                self.db
                    .sync_config_delete(CFG_LAST_ERROR)
                    .map_err(map_storage_error)?;
                self.db
                    .sync_config_delete(CFG_NEEDS_INVITE)
                    .map_err(map_storage_error)?;
                self.reconcile_chunks()?;
                self.events.push(Event::SyncChanged);
                self.events.push(Event::SyncRequested);
            }
            None => {
                self.db
                    .sync_config_delete(CFG_SERVER_URL)
                    .map_err(map_storage_error)?;
                self.sync_forget_remote_state()?;
                self.chunk_sync_on_disconnect()?;
                self.sync.invite_token = None;
                self.sync.state = SyncState::Disabled;
                self.sync.last_error = None;
                self.events.push(Event::SyncChanged);
            }
        }
        Ok(())
    }

    /// Drops everything about a server's feed: cursor, chain hash, epoch, seen
    /// sequences, heads, pending items; documents stay. Everything becomes unpublished.
    fn sync_forget_remote_state(&mut self) -> Result<(), ApiError> {
        for key in [
            CFG_SERVER_EPOCH,
            CFG_LAST_ERROR,
            CFG_LAST_PULL,
            CFG_LAST_PUSH,
            CFG_NEEDS_INVITE,
        ]
        .into_iter()
        .chain(LEGACY_V1_KEYS)
        {
            self.db.sync_config_delete(key).map_err(map_storage_error)?;
        }
        self.db.sync_seen_clear().map_err(map_storage_error)?;
        self.db.sync_heads_clear().map_err(map_storage_error)?;
        self.db.sync_pending_clear().map_err(map_storage_error)?;
        self.db
            .sync_doc_state_reset_all()
            .map_err(map_storage_error)?;
        self.db
            .keyring_mark_all_unpublished()
            .map_err(map_storage_error)?;
        self.db
            .device_log_mark_all_unpublished()
            .map_err(map_storage_error)?;
        self.feeds_forget_remote_state()?;
        Ok(())
    }

    /// The invite token given with the last `SyncConfigure`, once (the driver uses it to
    /// create the account on the server).
    pub fn sync_take_invite_token(&mut self) -> Result<Option<String>, ApiError> {
        Ok(self.sync.invite_token.take())
    }

    /// The driver found that the server does not know the account and it has no invite
    /// token: the status asks for one until the next `SyncConfigure` brings it.
    pub fn sync_account_unknown(&mut self) -> Result<(), ApiError> {
        // The cycle that found the account unknown had taken no token; if a `SyncConfigure` with one landed while it
        // ran, the next cycle uses it and nothing is asked for (the late report would otherwise set the flag after the
        // configure cleared it, and nothing would clear it again once the account is made).
        if self.sync.invite_token.is_some() {
            return Ok(());
        }
        self.db
            .sync_config_set(CFG_NEEDS_INVITE, "1")
            .map_err(map_storage_error)?;
        self.sync_set_state(SyncState::Error, Some(ACCOUNT_UNKNOWN_MESSAGE.to_owned()))
    }

    /// The configured server.
    pub(crate) fn sync_server_url(&self) -> Result<Option<String>, ApiError> {
        self.cfg(CFG_SERVER_URL)
    }

    /// The sync status.
    pub fn sync_info(&self) -> Result<SyncInfo, ApiError> {
        let server_url = self.sync_server_url()?;
        let configured = server_url.is_some();
        let states: HashMap<DocId, i64> = self
            .db
            .sync_doc_state_all()
            .map_err(map_storage_error)?
            .into_iter()
            .map(|s| (s.doc_id, s.pushed_up_to_seq))
            .collect();
        // Once the device has feed rows the counts are per feed; before its first cycle there is the
        // personal feed's per-document state alone.
        let feed_rows = self.db.sync_feed_list().map_err(map_storage_error)?;
        let feeds = if feed_rows.is_empty() {
            Vec::new()
        } else {
            self.feeds_info()?
        };
        let any_feed_fork = feed_rows.iter().any(|row| row.fork);
        let mut pending = 0usize;
        if feeds.is_empty() {
            for row in self.db.docs_all().map_err(map_storage_error)? {
                let pushed = states.get(&row.doc_id).copied().unwrap_or(0);
                let last = self
                    .db
                    .update_stats(row.doc_id)
                    .map_err(map_storage_error)?
                    .last_seq
                    .unwrap_or(0);
                if last > pushed {
                    pending += 1;
                }
            }
            if !self
                .db
                .keyring_unpublished()
                .map_err(map_storage_error)?
                .is_empty()
            {
                pending += 1;
            }
        } else {
            pending = feeds
                .iter()
                .map(|feed| usize::try_from(feed.pending_entries).unwrap_or(usize::MAX))
                .fold(0usize, usize::saturating_add);
        }
        let fork = any_feed_fork;
        let remote_cursor = match self.personal_feed()? {
            Some(personal) => feed_rows
                .iter()
                .find(|row| row.feed == personal)
                .map_or(0, |row| row.cursor),
            None => 0,
        };
        let state = if !configured {
            SyncState::Disabled
        } else if fork {
            SyncState::Forked
        } else {
            self.sync.state
        };
        let devices_seen = self
            .db
            .sync_heads_all()
            .map_err(map_storage_error)?
            .iter()
            .filter(|h| h.device_id != self.device.device_id)
            .count();
        // Document items only: a keyring entry parked for an earlier root key waits
        // for nothing a snapshot brings.
        let mut parked = self
            .db
            .sync_pending_all()
            .map_err(map_storage_error)?
            .iter()
            .filter(|row| row.doc_id != KEYRING_DOC_ID)
            .count();
        let personal_feed = self.personal_feed()?;
        for row in feed_rows
            .iter()
            .filter(|row| Some(row.feed) != personal_feed)
        {
            parked += self
                .db
                .sync_pending_all_in(row.feed)
                .map_err(map_storage_error)?
                .iter()
                .filter(|pending| pending.doc_id != KEYRING_DOC_ID)
                .count();
        }
        Ok(SyncInfo {
            configured,
            server_url,
            state,
            pending_entries: u32::try_from(pending).unwrap_or(u32::MAX),
            parked_items: u32::try_from(parked).unwrap_or(u32::MAX),
            remote_cursor,
            server_epoch: self.cfg(CFG_SERVER_EPOCH)?,
            last_push_at: self.cfg_i64(CFG_LAST_PUSH)?,
            last_pull_at: self.cfg_i64(CFG_LAST_PULL)?,
            last_error: self.sync.last_error.clone().or(self.cfg(CFG_LAST_ERROR)?),
            fork_detected: fork,
            devices_seen: u32::try_from(devices_seen).unwrap_or(u32::MAX),
            needs_invite_token: configured && self.cfg(CFG_NEEDS_INVITE)?.as_deref() == Some("1"),
            transfers: self.transfers_info()?,
            feeds,
        })
    }

    /// The driver reports where it stands; emits `SyncChanged`. `Idle` means a whole
    /// cycle succeeded and stamps `last_pull_at`.
    pub fn sync_set_state(
        &mut self,
        state: SyncState,
        error: Option<String>,
    ) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        if state == SyncState::Idle {
            self.db
                .sync_config_set(CFG_LAST_PULL, &now.to_string())
                .map_err(map_storage_error)?;
        }
        match &error {
            Some(message) => self
                .db
                .sync_config_set(CFG_LAST_ERROR, message)
                .map_err(map_storage_error)?,
            None => self
                .db
                .sync_config_delete(CFG_LAST_ERROR)
                .map_err(map_storage_error)?,
        }
        self.sync.state = state;
        self.sync.last_error = error;
        self.events.push(Event::SyncChanged);
        Ok(())
    }

    fn cfg(&self, key: &str) -> Result<Option<String>, ApiError> {
        self.db.sync_config_get(key).map_err(map_storage_error)
    }

    fn cfg_i64(&self, key: &str) -> Result<Option<i64>, ApiError> {
        Ok(self.cfg(key)?.and_then(|v| v.parse().ok()))
    }

    pub(crate) fn server_epoch(&self) -> Result<Option<[u8; 16]>, ApiError> {
        Ok(self
            .cfg(CFG_SERVER_EPOCH)?
            .and_then(|v| decode_hex(&v))
            .and_then(|v| v.try_into().ok()))
    }

    // ----- identity and keys --------------------------------------------------------------

    /// The account id and this device's id, once a kit exists.
    pub fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        Ok(self
            .security
            .log_state
            .as_ref()
            .map(|state| (state.account_id, self.device.device_id)))
    }

    pub(crate) fn signer(&self) -> &SigningKey {
        self.security.device_keys.signing()
    }

    /// Signs an authentication message with the device signing key.
    pub fn sync_sign(&self, message: &[u8]) -> Result<Signature, ApiError> {
        Ok(kb_sync::sign_auth(message, self.signer()))
    }

    /// The signing key of a device of the account, revoked or not: feed items keep
    /// their signatures after their author's revocation (the server stops taking new
    /// ones from it; what it took before stays valid).
    pub(crate) fn device_verifier(&self, device_id: DeviceId) -> Result<VerifyingKey, ApiError> {
        self.security
            .log_state
            .as_ref()
            .and_then(|state| state.devices.iter().find(|d| d.device_id == device_id))
            .map(|record| record.keys.signing)
            .ok_or_else(|| {
                ApiError::crypto("the entry was signed by a device the log does not know")
            })
    }

    /// The keyring key of the current root key (sealing) followed by those of the
    /// retained previous ones (opening the feed's past).
    pub(crate) fn keyring_keys(&self) -> Vec<Secret32> {
        let mut keys = vec![kb_sync::keyring_key(&self.ark)];
        keys.extend(
            self.security
                .previous_arks
                .iter()
                .rev()
                .map(|(_, ark)| kb_sync::keyring_key(ark)),
        );
        keys
    }

    /// The account document key of a root key generation: the current one or a
    /// retained previous one; `None` for a generation this device never had.
    pub(crate) fn account_doc_key_for(&self, generation: u32) -> Option<Secret32> {
        let ark = if generation == self.security.ark_generation {
            &self.ark
        } else {
            &self
                .security
                .previous_arks
                .iter()
                .find(|(g, _)| *g == generation)?
                .1
        };
        Some(kb_crypto::derive_key(ark.secret(), Label::ACCOUNT_DOC, b""))
    }

    /// The key to seal a known document under now, with the epoch the header carries:
    /// the account key of the current generation, or the document key of the space's
    /// current epoch (created on first use, ADR-0022). `None` while the space has no
    /// key record yet.
    pub(crate) fn current_doc_key(
        &mut self,
        row: &DocRow,
    ) -> Result<Option<(Secret32, u32)>, ApiError> {
        if row.kind == DocKindCode::Account {
            let generation = self.security.ark_generation;
            return Ok(self
                .account_doc_key_for(generation)
                .map(|k| (k, generation)));
        }
        let Some(space_id) = row.space_id else {
            return Ok(None);
        };
        // A page another device moved here (ADR-0038): this space has no record of it yet, and
        // only the moving device creates one. The newest record the document has in another
        // space serves until that one arrives (every reader tries every space's record).
        if self.doc_record_is_elsewhere(space_id, row.doc_id) {
            return self.newest_doc_key_elsewhere(space_id, row.doc_id);
        }
        if self.security.keyring.space_records(space_id).is_empty() {
            return Ok(None);
        }
        self.ensure_doc_key_of(row)?;
        let epoch = self.space_epoch(space_id);
        Ok(self
            .doc_key_in(space_id, row.doc_id, epoch)?
            .map(|key| (key, epoch)))
    }

    /// The key of the newest `Doc` record the document has outside `space_id`, with its epoch.
    /// "Newest": the latest creation time, then the higher epoch, then the greater space id; a
    /// record whose space key this device lacks is skipped.
    fn newest_doc_key_elsewhere(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
    ) -> Result<Option<(Secret32, u32)>, ApiError> {
        let mut candidates: Vec<(i64, u32, SpaceId)> = self
            .security
            .keyring
            .doc_records(doc_id)
            .into_iter()
            .filter(|record| record.space_id != space_id)
            .filter_map(|record| match record.subject {
                KeySubject::Doc { epoch, .. } => Some((record.created_at, epoch, record.space_id)),
                _ => None,
            })
            .collect();
        candidates.sort_unstable_by(|a, b| b.cmp(a));
        for (_, epoch, space) in candidates {
            if let Ok(Some(key)) = self.doc_key_in(space, doc_id, epoch) {
                return Ok(Some((key, epoch)));
            }
        }
        Ok(None)
    }

    /// The document key of an epoch, when the keyring has the record and the space key
    /// of that epoch.
    pub(crate) fn doc_key_in(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
        epoch: u32,
    ) -> Result<Option<Secret32>, ApiError> {
        let subject = KeySubject::Doc { doc_id, epoch };
        let Some(record) = self.security.keyring.record(space_id, &subject).cloned() else {
            return Ok(None);
        };
        let Some(space_key) = self.space_key_at(space_id, epoch)? else {
            return Ok(None);
        };
        Ok(Some(
            kr::unwrap_doc_key(&space_key, &record).map_err(map_keys_error)?,
        ))
    }

    /// The keys of a document this device does not know: the records that name it at
    /// `epoch`, in the space the page index puts it in, and in whichever space the
    /// keyring holds one.
    ///
    /// Every space that holds a record of the document at `epoch` contributes its key (a page
    /// that moved between spaces has records in several; the entry's AEAD decides which opens
    /// it), the page index's space first.
    pub(crate) fn guess_keys(
        &mut self,
        doc_id: DocId,
        epoch: u32,
    ) -> Result<Vec<Secret32>, ApiError> {
        let mut keys = Vec::new();
        let mut tried: Vec<SpaceId> = Vec::new();
        if let Ok(space_id) = self.space_of_page(PageId::from_bytes(doc_id.to_bytes())) {
            tried.push(space_id);
            if let Some(key) = self.doc_key_in(space_id, doc_id, epoch)? {
                keys.push(key);
            }
        }
        if self.sync.known_spaces.is_none() {
            self.refresh_known_spaces();
        }
        let spaces = self.sync.known_spaces.clone().unwrap_or_default();
        for space_id in spaces {
            if tried.contains(&space_id) {
                continue;
            }
            if let Some(key) = self.doc_key_in(space_id, doc_id, epoch)? {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    /// The spaces the keyring knows, computed once per batch (listing the items clones
    /// them all; ten thousand documents make that expensive per lookup).
    fn refresh_known_spaces(&mut self) {
        let spaces: BTreeSet<SpaceId> = self
            .security
            .keyring
            .items()
            .iter()
            .map(KeyringItem::space_id)
            .chain(self.docs.structures.keys().copied())
            .collect();
        self.sync.known_spaces = Some(spaces.into_iter().collect());
    }

    // ----- device log ---------------------------------------------------------------------

    /// Encoded device log entries and how many are published.
    pub fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        let all = self.db.device_log_all().map_err(map_storage_error)?;
        let unpublished = self
            .db
            .device_log_unpublished()
            .map_err(map_storage_error)?;
        let published = all.len().saturating_sub(unpublished.len());
        Ok((
            all.into_iter().map(|row| row.entry).collect(),
            u64::try_from(published).unwrap_or(u64::MAX),
        ))
    }

    /// The server holds the log up to `up_to_seq`.
    pub fn sync_log_published(&mut self, up_to_seq: u64) -> Result<(), ApiError> {
        self.db
            .device_log_mark_published(up_to_seq)
            .map_err(map_storage_error)
    }

    /// The account's log as the server holds it: entries beyond ours are verified and
    /// appended (that is how this device learns the keys of the others); `true` when
    /// something was added. A log that does not extend ours is refused (the device
    /// must be restored again from a current backup).
    pub fn sync_log_received(&mut self, entries: &[Vec<u8>]) -> Result<bool, ApiError> {
        if let Some((me, _)) = self.sync_identity()? {
            // An item of a device this log does not list is no longer a race with a new device.
            self.shared.fresh_logs.insert(me);
        }
        let local = self.db.device_log_all().map_err(map_storage_error)?;
        for (ours, theirs) in local.iter().zip(entries) {
            if ours.entry != *theirs {
                return Err(ApiError::crypto(
                    "the server's device log does not match this device's",
                ));
            }
        }
        if entries.len() <= local.len() {
            return Ok(false);
        }
        let before = self.security.log_state.clone();
        let mut log = self.security.log.take().unwrap_or_default();
        let mut rows = Vec::new();
        let mut new_entries = Vec::new();
        for bytes in &entries[local.len()..] {
            let entry = LogEntry::decode(bytes).map_err(|e| map_keys_error(KeysError::Log(e)))?;
            rows.push(DeviceLogRow {
                seq: entry.seq,
                entry: entry.encode(),
                hash: entry.hash(),
            });
            new_entries.push(entry.clone());
            if let Err(e) = log.append(entry) {
                self.security.log = (!log.entries().is_empty()).then_some(log);
                return Err(map_keys_error(KeysError::Log(e)));
            }
        }
        let state = match log.verify() {
            Ok(state) => state,
            Err(e) => {
                self.security.log = (!log.entries().is_empty()).then_some(log);
                return Err(map_keys_error(KeysError::Log(e)));
            }
        };
        if local.is_empty() && state.active(self.device.device_id).is_none() {
            return Err(ApiError::crypto(
                "the server's device log does not list this device",
            ));
        }
        self.security.log = Some(log);
        let up_to = rows.last().map_or(0, |r| r.seq);
        self.db
            .transaction(|tx| {
                for row in &rows {
                    tx.device_log_append(row)?;
                }
                tx.device_log_mark_published(up_to)
            })
            .map_err(map_storage_error)?;
        self.security.log_state = Some(state.clone());
        self.events.push(Event::SecurityChanged);
        tracing::info!(entries = rows.len(), "device log extended from the server");
        self.after_log_change(before.as_ref(), &state, &new_entries)?;
        Ok(true)
    }

    // ----- epoch ----------------------------------------------------------------------------

    /// First contact or an epoch change; `true` when the core reset its cursor state
    /// because the server lost its data (everything becomes unpublished and every document
    /// gets a snapshot on its next upload).
    pub fn sync_remote_epoch(&mut self, epoch: [u8; 16]) -> Result<bool, ApiError> {
        match self.server_epoch()? {
            Some(current) if current == epoch => Ok(false),
            Some(_) => {
                tracing::warn!("the server's storage epoch changed: re-uploading everything");
                self.sync_forget_remote_state()?;
                self.chunk_sync_on_server_change()?;
                self.db
                    .sync_config_set(CFG_SERVER_EPOCH, &hex(&epoch))
                    .map_err(map_storage_error)?;
                self.events.push(Event::SyncChanged);
                Ok(true)
            }
            None => {
                self.db
                    .sync_config_set(CFG_SERVER_EPOCH, &hex(&epoch))
                    .map_err(map_storage_error)?;
                Ok(false)
            }
        }
    }

    // ----- push -----------------------------------------------------------------------------

    /// The whole document as one encoded snapshot blob (importable like an update).
    pub(crate) fn full_snapshot_blob(&self, row: &DocRow) -> Result<Vec<u8>, ApiError> {
        if row.kind == DocKindCode::Comments {
            return self.comment_stream_snapshot_of_doc(row);
        }
        match self.open_doc(row) {
            Some(doc) => doc.export_snapshot().map_err(map_doc_error),
            None => documents::load_doc(&self.db, row, engine_kind(row.kind), self.rng.as_ref())?
                .export_snapshot()
                .map_err(map_doc_error),
        }
    }

    fn doc_state(&self, doc_id: DocId) -> Result<SyncDocState, ApiError> {
        Ok(self
            .db
            .sync_doc_state_get(doc_id)
            .map_err(map_storage_error)?
            .unwrap_or_else(|| doc_state_default(doc_id)))
    }

    /// The last local update sequence the server holds for a document, for the
    /// compaction guard: unpushed updates must stay in the log. `None` without a server.
    pub(crate) fn sync_pushed_up_to(&self, doc_id: DocId) -> Result<Option<i64>, ApiError> {
        if self.sync_server_url()?.is_none() {
            return Ok(None);
        }
        // A document of a shared space is pushed into that space's feed.
        let Some(personal) = self.personal_feed()? else {
            return Ok(Some(self.doc_state(doc_id)?.pushed_up_to_seq));
        };
        let feed = match self.db.doc_get(doc_id).map_err(map_storage_error)? {
            Some(row) => Self::feed_of_doc(&row, &self.shared_space_map()?, personal),
            None => Some(personal),
        };
        let Some(feed) = feed else {
            return Ok(None);
        };
        Ok(Some(self.doc_state_in(feed, doc_id)?.pushed_up_to_seq))
    }

    /// Marks documents for a snapshot at the next cycle (`sync_snapshot_candidates`): the
    /// entries since their last snapshot count as past the threshold. A page that moved to
    /// another space (ADR-0038) is snapshotted under its new key, which makes its earlier
    /// entries (sealed under the old space's key) unnecessary to a device that has not synced
    /// them.
    pub(crate) fn sync_mark_for_snapshot(&mut self, docs: &[DocId]) -> Result<(), ApiError> {
        for doc_id in docs {
            let mut state = self.doc_state(*doc_id)?;
            if state.entries_since_snapshot >= SNAPSHOT_AFTER_ENTRIES {
                continue;
            }
            state.entries_since_snapshot = SNAPSHOT_AFTER_ENTRIES;
            self.db
                .sync_doc_state_put(&state)
                .map_err(map_storage_error)?;
        }
        Ok(())
    }

    // ----- pull -----------------------------------------------------------------------------

    /// The keys a known document's item may have been sealed under: the account key of that
    /// root key generation, or the document key of that space epoch. A page that moved
    /// between spaces (ADR-0038) may have been sealed under the record of either space: the
    /// record of the document's own space comes first, then those of the other spaces that
    /// hold one at that epoch (the AEAD decides which opens the item).
    pub(crate) fn reading_keys(
        &mut self,
        row: &DocRow,
        key_epoch: u32,
    ) -> Result<Vec<Secret32>, ApiError> {
        if row.kind == DocKindCode::Account {
            return Ok(self.account_doc_key_for(key_epoch).into_iter().collect());
        }
        let Some(space_id) = row.space_id else {
            return Ok(Vec::new());
        };
        let mut keys = Vec::new();
        if let Some(key) = self.doc_key_in(space_id, row.doc_id, key_epoch)? {
            keys.push(key);
        }
        for other in self.security.keyring.doc_spaces(row.doc_id) {
            // A record of another space that does not unwrap is skipped: the document's own
            // record, or another one, may still open the item.
            if other != space_id
                && let Ok(Some(key)) = self.doc_key_in(other, row.doc_id, key_epoch)
            {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    /// `false` when the blobs depend on history this device lacks: nothing is persisted
    /// then and the caller parks the entry.
    pub(crate) fn apply_account_updates(
        &mut self,
        blobs: &[Vec<u8>],
        now: i64,
    ) -> Result<bool, ApiError> {
        self.ensure_account()?;
        let account = self
            .docs
            .account
            .as_mut()
            .ok_or_else(|| ApiError::internal("account document missing"))?;
        let mut complete = true;
        for blob in blobs {
            complete &= account
                .model
                .doc()
                .import_checked(blob)
                .map_err(map_doc_error)?;
        }
        if !complete {
            return Ok(false);
        }
        account.model.refresh().map_err(map_model_error)?;
        let doc_id = account.doc_id;
        self.db
            .transaction(|tx| {
                for blob in blobs {
                    tx.update_append(doc_id, blob, UpdateOrigin::Remote, now)?;
                }
                Ok::<(), StorageError>(())
            })
            .map_err(map_storage_error)?;
        if let Some(account) = self.docs.account.as_mut() {
            account.last_persisted = account.model.doc().version();
        }
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::SecurityChanged);
        Ok(true)
    }

    /// `false` when the blobs depend on history this device lacks (nothing persisted).
    pub(crate) fn apply_structure_updates(
        &mut self,
        doc_id: DocId,
        blobs: &[Vec<u8>],
        now: i64,
        notify: Option<AccountId>,
    ) -> Result<bool, ApiError> {
        let known = self
            .docs
            .structures
            .iter()
            .find(|(_, open)| open.doc_id == doc_id)
            .map(|(space, _)| *space);
        let space_id = match known {
            Some(space) => space,
            None => self.create_space_from_remote(doc_id, blobs, now)?,
        };
        let open = self.structure_mut(space_id)?;
        // The view links and the look of their pages (M13 slice 4), compared after the import.
        let links_before = crate::links::link_snapshot(&open.model);
        let ids_before: HashSet<PageId> = open.model.all_page_ids().into_iter().collect();
        let titles_before = titles_of(&open.model);
        let live_before = open.model.live_page_count();
        // What the update may change in the properties (ADR-0026): compared before and after, so
        // that only the rows of the pages that changed are rewritten, and the whole space only
        // when the schema changed.
        let schema_before = open.model.property_schema();
        let values_before: HashMap<PageId, kb_model::PageIndexEntry> =
            open.model.property_index_all().into_iter().collect();
        if known.is_some() {
            let mut complete = true;
            for blob in blobs {
                complete &= open
                    .model
                    .doc()
                    .import_checked(blob)
                    .map_err(map_doc_error)?;
            }
            if !complete {
                return Ok(false);
            }
            open.model.refresh().map_err(map_model_error)?;
        }
        let order_after = open.model.all_page_ids();
        let ids_after: HashSet<PageId> = order_after.iter().copied().collect();
        // Pages that became residue (they moved to another space, ADR-0038) left this document
        // without being purged: no crypto-erase, no removal of their derived rows below.
        let mut residue: HashSet<PageId> = open
            .model
            .moved_out()
            .into_iter()
            .map(|(page, _, _)| page)
            .collect();
        // A page another device created under one that moved away is hidden until `reconcile_moves`
        // re-roots it: not gone either.
        residue.extend(open.model.orphans());
        let titles_after = titles_of(&open.model);
        let live_after = open.model.live_page_count();
        let schema_changed = known.is_none() || open.model.property_schema() != schema_before;
        let values_after: HashMap<PageId, kb_model::PageIndexEntry> =
            open.model.property_index_all().into_iter().collect();
        let changed_values: Vec<PageId> = values_before
            .keys()
            .chain(values_after.keys())
            .copied()
            .filter(|page| values_before.get(page) != values_after.get(page))
            .collect::<HashSet<PageId>>()
            .into_iter()
            .collect();
        // `persist_structure_with` exports what the model has beyond `last_persisted` (the
        // imported remote updates, re-encoded), stores it with the remote origin, rewrites the
        // page index of the space and the derived property rows that changed.
        let props = if known.is_none() || schema_changed {
            crate::properties::PropsScope::Space
        } else {
            crate::properties::PropsScope::Pages(&changed_values)
        };
        // Pages that arrive here from another space (a move that happened elsewhere): found
        // before the write, which replaces the page index rows that name their old space.
        let candidates: Vec<PageId> = order_after
            .iter()
            .copied()
            .filter(|page| known.is_none() || !ids_before.contains(page))
            .collect();
        let arrivals = self.find_arrivals(space_id, &candidates)?;
        self.persist_structure_with(space_id, None, UpdateOrigin::Remote, props)?;
        // Derived text: title rows for every page; bodies of pages that exist locally
        // (a page whose content has not arrived gets no empty document created here).
        for (page, title) in &titles_after {
            let is_new = !ids_before.contains(page);
            let renamed = titles_before.get(page) != Some(title);
            if is_new {
                self.text_after_create(*page, title)?;
                if self
                    .db
                    .doc_get(DocId::for_page(*page))
                    .map_err(map_storage_error)?
                    .is_some()
                {
                    self.refresh_page_text(*page)?;
                }
            } else if renamed {
                self.text_after_rename(*page, title)?;
            }
        }
        let gone: Vec<PageId> = ids_before
            .difference(&ids_after)
            .filter(|page| !residue.contains(page))
            .copied()
            .collect();
        if !gone.is_empty() {
            self.text_after_remove(&gone)?;
        }
        if schema_changed {
            self.events.push(Event::PropertySchemaChanged { space_id });
        }
        self.events.push(Event::StructureChanged { space_id });
        if live_before != live_after || known.is_none() {
            self.events.push(Event::SpacesChanged);
        }
        self.view_links_structure_applied(space_id, &links_before);
        self.settle_arrivals(space_id, &arrivals)?;
        // A remote update can leave a page in two spaces (two devices moved it), a page created
        // under one that moved away, or the same attachment in two spaces: one result everywhere.
        self.reconcile_moves()?;
        self.attachments_arrived()?;
        // Being added to a Person property by somebody else's entry (ADR-0049): a notification. A
        // structure this device learns of for the first time is history, not news.
        if let Some(actor) = notify
            && known.is_some()
        {
            self.person_assignments_notify(space_id, &values_before, &values_after, actor, now)?;
        }
        Ok(true)
    }

    /// A structure document arriving for a space this device has never seen.
    fn create_space_from_remote(
        &mut self,
        doc_id: DocId,
        blobs: &[Vec<u8>],
        now: i64,
    ) -> Result<SpaceId, ApiError> {
        let doc = Doc::load(
            DocKind::Structure,
            documents::new_peer(self.rng.as_ref()),
            None,
            blobs,
        )
        .map_err(map_doc_error)?;
        let model = StructureDoc::from_doc(doc).map_err(map_model_error)?;
        let space_id = model.space().id;
        if self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::storage(
                "a second structure document for a known space was refused",
            ));
        }
        let row = documents::new_doc_row(doc_id, DocKindCode::Structure, Some(space_id), now);
        self.db
            .transaction(|tx| {
                tx.doc_insert(&row)?;
                for blob in blobs {
                    tx.update_append(doc_id, blob, UpdateOrigin::Remote, now)?;
                }
                Ok::<(), StorageError>(())
            })
            .map_err(map_storage_error)?;
        let last_persisted = model.doc().version();
        self.docs.structures.insert(
            space_id,
            OpenStructure {
                doc_id,
                model,
                last_persisted,
            },
        );
        self.ensure_account()?;
        let listed = self
            .docs
            .account
            .as_ref()
            .is_some_and(|a| a.model.spaces().contains(&space_id));
        if !listed {
            if let Some(account) = self.docs.account.as_ref() {
                account.model.add_space(space_id).map_err(map_model_error)?;
            }
            self.persist_account()?;
        }
        self.sync.known_spaces = None;
        tracing::info!(space = %short_id(&space_id), "space created from the feed");
        Ok(space_id)
    }

    /// `false` when the blobs depend on history this device lacks (nothing persisted).
    pub(crate) fn apply_page_updates(
        &mut self,
        page_id: PageId,
        blobs: &[Vec<u8>],
        now: i64,
        notify: Option<AccountId>,
        writer: Option<Writer>,
    ) -> Result<bool, ApiError> {
        let doc_id = DocId::for_page(page_id);
        let space_id = self.space_of_page(page_id)?;
        let was_open = self.docs.pages.contains_key(&page_id);
        let row = match self.db.doc_get(doc_id).map_err(map_storage_error)? {
            Some(row) => row,
            None => {
                let row = documents::new_doc_row(doc_id, DocKindCode::Page, Some(space_id), now);
                self.db.doc_insert(&row).map_err(map_storage_error)?;
                row
            }
        };
        let mut complete = true;
        // The document's version before the entry, for the page history journal.
        let before: Frontiers;
        if let Some(open) = self.docs.pages.get_mut(&page_id) {
            before = open.model.doc().frontiers();
            for blob in blobs {
                complete &= open
                    .model
                    .doc()
                    .import_checked(blob)
                    .map_err(map_doc_error)?;
            }
            if !complete {
                return Ok(false);
            }
        } else {
            // Not open in an editor: a transient load validates the blobs and feeds the
            // text index, then goes away again.
            let doc = documents::load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
            before = doc.frontiers();
            for blob in blobs {
                complete &= doc.import_checked(blob).map_err(map_doc_error)?;
            }
            if !complete {
                return Ok(false);
            }
            let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
            let last_persisted = model.doc().version();
            self.docs.pages.insert(
                page_id,
                OpenPage {
                    doc_id,
                    model,
                    last_persisted,
                },
            );
        }
        // The entry is a session of its writer in the page's history (ADR-0057); a snapshot has no
        // author and is left out (its history stays undated). The row rides in the transaction below.
        let journal_op = writer.and_then(|writer| {
            let after = self
                .docs
                .pages
                .get(&page_id)
                .map(|open| open.model.doc().frontiers())?;
            self.journal_plan(
                &JournalWrite {
                    page_id,
                    before: &before,
                    after: &after,
                    writer,
                    kind: JournalKind::Edit,
                },
                self.page_title(page_id).ok(),
            )
        });
        self.db
            .transaction(|tx| {
                for blob in blobs {
                    tx.update_append(doc_id, blob, UpdateOrigin::Remote, now)?;
                }
                crate::history::record_journal(tx, page_id, journal_op.as_ref());
                Ok::<(), StorageError>(())
            })
            .map_err(map_storage_error)?;
        if let Some(open) = self.docs.pages.get_mut(&page_id) {
            open.last_persisted = open.model.doc().version();
        }
        // The mentions of people the page had before this entry (the diff that notifies, ADR-0049).
        let old_mentions = if notify.is_some() {
            self.db
                .person_mentions_of(doc_id)
                .map_err(map_storage_error)?
        } else {
            Vec::new()
        };
        self.refresh_page_text(page_id)?;
        if let Some(actor) = notify {
            self.person_mentions_notify(space_id, page_id, &old_mentions, actor, now)?;
        }
        if !was_open {
            self.docs.pages.remove(&page_id);
        }
        // A shared space's records come from its feed; this device makes none for a page it was told of.
        if self.shared_feed_of(space_id)?.is_none() {
            self.ensure_doc_key(space_id, doc_id)?;
        }
        self.events.push(Event::PageContentChanged {
            page_id,
            source: ChangeSource::Remote,
        });
        // View links (M13 slice 4): an entry applied from the feed refreshes the page's links on this
        // device too (when its role is Editor or Owner). Best effort.
        let _ = self.view_links_page_changed(page_id, ChangeSource::Remote);
        Ok(true)
    }

    // ----- snapshots --------------------------------------------------------------------

    pub(crate) fn open_doc(&self, row: &DocRow) -> Option<&Doc> {
        match row.kind {
            DocKindCode::Account => self.docs.account.as_ref().map(|a| a.model.doc()),
            DocKindCode::Structure => row
                .space_id
                .and_then(|space| self.docs.structures.get(&space))
                .map(|s| s.model.doc()),
            DocKindCode::Page => self
                .docs
                .pages
                .get(&PageId::from_bytes(row.doc_id.to_bytes()))
                .map(|p| p.model.doc()),
            // A comment stream is not an open engine document: its snapshot is the records snapshot
            // (`comment_stream_snapshot`).
            DocKindCode::Comments => None,
        }
    }

    // ----- heads ------------------------------------------------------------------------

    // ----- hooks ------------------------------------------------------------------------

    /// After a local change that sync should carry: asks the driver for a cycle (once
    /// per drained batch of events).
    pub(crate) fn sync_note_local_change(&mut self) {
        if self.sync.state != SyncState::Disabled
            && !self
                .events
                .iter()
                .any(|e| matches!(e, Event::SyncRequested))
        {
            self.events.push(Event::SyncRequested);
        }
    }

    /// At open: the in-memory state from the configuration.
    pub(crate) fn sync_load(&mut self) -> Result<(), ApiError> {
        self.sync.state = if self.sync_server_url()?.is_some() {
            let forked = self
                .db
                .sync_feed_list()
                .map_err(map_storage_error)?
                .iter()
                .any(|row| row.fork);
            if forked {
                SyncState::Forked
            } else {
                SyncState::Idle
            }
        } else {
            SyncState::Disabled
        };
        self.sync.last_error = self.cfg(CFG_LAST_ERROR)?;
        Ok(())
    }
}

/// A document that sync has not touched: nothing pushed, the first `device_seq` next.
pub(crate) fn doc_state_default(doc_id: DocId) -> SyncDocState {
    SyncDocState {
        doc_id,
        pushed_up_to_seq: 0,
        next_device_seq: 1,
        snapshot_covers: None,
        entries_since_snapshot: 0,
        bytes_since_snapshot: 0,
        remote_snapshot_covers: None,
    }
}

fn titles_of(model: &StructureDoc) -> HashMap<PageId, String> {
    model
        .all_page_ids()
        .into_iter()
        .filter_map(|id| model.page(id).map(|info| (id, info.title)))
        .collect()
}
