//! Keys and identity (`docs/development/milestone-04.md`, "Core"): the locked core
//! state, device keys, the device log, the recovery kit, the keyring, root key
//! rotation and the app lock. The backup file lives in `backup.rs`.
//!
//! Root key rotation is a two-phase protocol so that a crash at any point leaves a
//! state the next open repairs: the new ARK is parked in the keystore (`ark-next`),
//! the database rows that depend on it are written, the database is rekeyed, the
//! keystore is promoted, and a `rotation_pending` flag in `meta` tells the next open
//! whether the last step (the new recovery blob) completed. See `rotate_ark`.

use std::collections::{HashMap, HashSet};

use kb_crypto::{Argon2Params, Secret32, WrappedKey};
use kb_keys::keyring as kr;
use kb_keys::lock as lk;
use kb_keys::log as lg;
use kb_keys::recovery::RecoveryBlob;
use kb_keys::rotation::RotationBox;
use kb_keys::{
    Ark, ArkState, DeviceKeys, DeviceLog, KeySubject, Keyring, KeyringItem, KeysError, Keystore,
    LockedArk, LogEntry, LogState, RecoveryKey,
};
use kb_storage::{
    Db, DeviceLogRow, DocKindCode, DocQueries, FeedQueries, KeyringRow, SecurityQueries,
    StorageError, SyncQueries,
};
use kb_types::sync::FeedKind;
use kb_types::{
    ApiError, AppLockInfo, DeviceEntry, DeviceId, DocId, ErrorCode, Event, FeedId, InstallId,
    KeyringInfo, MemberRole, RecoveryKit, RecoveryKitInfo, Request, Response, SecurityInfo,
    SpaceId,
};

use crate::{Core, CoreDeps, map_keys_error, map_storage_error, short_id};

/// The space key epoch of this milestone (epochs bump with revocation, M7).
pub(crate) const SPACE_EPOCH: u32 = 0;

// `meta` keys of the security state.
pub(crate) const META_ACCOUNT_ID: &str = "account_id";
pub(crate) const META_RECOVERY_ID: &str = "recovery_id";
pub(crate) const META_RECOVERY_BLOB: &str = "recovery_blob";
pub(crate) const META_RECOVERY_CREATED: &str = "recovery_created_at";
pub(crate) const META_RECOVERY_VERIFIED: &str = "recovery_verified_at";
pub(crate) const META_RECOVERY_REISSUED: &str = "recovery_reissued_at";
pub(crate) const META_ARK_GENERATION: &str = "ark_generation";
pub(crate) const META_ROTATION_PENDING: &str = "rotation_pending";
pub(crate) const META_RECOVERY_STALE: &str = "recovery_stale";
/// Previous root keys by generation, `gen:hex,...` (reading keys, ADR-0022).
pub(crate) const META_PREVIOUS_ARKS: &str = "previous_arks";
/// Unix ms when this device learned it was revoked.
pub(crate) const META_REVOKED_AT: &str = "revoked_at";
/// A `RootRotated` box for this device waiting for the passphrase: `gen:hex`.
pub(crate) const META_PENDING_ROTATION: &str = "pending_rotation";
/// `1` when the current recovery blob is on the configured server.
pub(crate) const META_RECOVERY_PUBLISHED: &str = "recovery_published";
/// Recovery ids the server still holds and should delete, hex, comma-separated.
pub(crate) const META_RECOVERY_OBSOLETE: &str = "recovery_obsolete";
/// How many previous root keys a device keeps.
pub(crate) const PREVIOUS_ARKS_KEPT: usize = 8;

/// Keystore entry parking the next ARK during a rotation (plain).
fn next_entry_name(install_id: &InstallId) -> String {
    format!("ark-next:{install_id}")
}

/// Keystore entry parking the next ARK during a rotation while the lock is on.
fn next_locked_entry_name(install_id: &InstallId) -> String {
    format!("ark-next-locked:{install_id}")
}

/// Keystore subject of the parked locked blob.
fn next_locked_subject(install_id: &InstallId) -> Vec<u8> {
    let mut subject = b"ark-next-locked".to_vec();
    subject.extend_from_slice(install_id.as_bytes());
    subject
}

/// What `meta` knows about the recovery kit.
#[derive(Clone)]
pub(crate) struct RecoveryMeta {
    pub(crate) id: [u8; 32],
    pub(crate) blob: WrappedKey,
    pub(crate) created_at: i64,
    pub(crate) verified_at: Option<i64>,
    pub(crate) reissued_at: Option<i64>,
}

/// Security state of an open core.
pub(crate) struct Security {
    pub(crate) device_keys: DeviceKeys,
    pub(crate) log: Option<DeviceLog>,
    pub(crate) log_state: Option<LogState>,
    pub(crate) keyring: Keyring,
    /// Unwrapped space keys by space and epoch.
    pub(crate) space_keys: HashMap<(SpaceId, u32), Secret32>,
    /// Previous root keys by generation: reading keys for the feed's past (ADR-0022).
    pub(crate) previous_arks: Vec<(u32, Ark)>,
    /// A `RootRotated` box for this device (generation, sealed) that waits for the
    /// app-lock passphrase.
    pub(crate) pending_rotation: Option<(u32, Vec<u8>)>,
    /// This device was revoked from the account.
    pub(crate) revoked: bool,
    /// The current recovery blob is on the configured server.
    pub(crate) recovery_published: bool,
    /// Recovery ids the server still holds and should delete.
    pub(crate) recovery_obsolete: Vec<[u8; 32]>,
    pub(crate) recovery: Option<RecoveryMeta>,
    /// Argon2id parameters when the app lock is enabled.
    pub(crate) lock: Option<Argon2Params>,
    pub(crate) ark_generation: u32,
    /// The last rotation did not get to write the new recovery blob: the stored kit
    /// does not open the current root key; the user must re-issue it.
    pub(crate) recovery_stale: bool,
}

/// What [`Core::open_or_locked`] returns.
// Built once per open; a boxed core would only move the allocation into every shell.
#[allow(clippy::large_enum_variant)]
pub enum Opened {
    /// The workspace is open.
    Ready(Core),
    /// The ARK is wrapped under the app-lock passphrase; nothing is open yet.
    Locked(LockedCore),
}

/// A core whose root key is wrapped under the app-lock passphrase (ADR-0018). Holds
/// the platform dependencies and the wrapped blob, no key material, no database.
pub struct LockedCore {
    pub(crate) deps: CoreDeps,
    pub(crate) locked: LockedArk,
}

impl LockedCore {
    /// Derives the wrapping key from `passphrase`, unwraps the ARK and runs the normal
    /// open sequence. A wrong passphrase returns the locked core unchanged with an
    /// `InvalidInput` error for the field `passphrase`.
    // The caller gets its locked core back on failure; this runs once per unlock.
    #[allow(clippy::result_large_err)]
    pub fn unlock(self, passphrase: &str) -> Result<Core, (LockedCore, ApiError)> {
        let ark = match self
            .locked
            .unlock_with_minimum(passphrase, &self.deps.lock_params)
        {
            Ok(ark) => ark,
            Err(err) => return Err((self, map_keys_error(err))),
        };
        let params = self.locked.params();
        let LockedCore { deps, locked } = self;
        Core::open_with_ark(deps, ark, Some(params), Some(passphrase))
            .map_err(|(deps, err)| (LockedCore { deps, locked }, err))
    }

    /// The Argon2id parameters of the stored blob (for diagnostics).
    #[must_use]
    pub fn params(&self) -> Argon2Params {
        self.locked.params()
    }
}

/// Everything `prepare` builds before the `Core` exists.
struct Prepared {
    ark: Ark,
    db: Db,
    device: kb_storage::DeviceRow,
    docs: crate::documents::Documents,
    security: Security,
}

impl Core {
    /// Like [`Core::open`], but returns a [`LockedCore`] instead of failing when the
    /// app lock is enabled.
    pub fn open_or_locked(deps: CoreDeps) -> Result<Opened, ApiError> {
        let state =
            lk::load_ark_state(deps.keystore.as_ref(), &deps.install_id).map_err(map_keys_error)?;
        match state {
            ArkState::Missing => {
                let ark =
                    lk::create_ark(deps.keystore.as_ref(), deps.rng.as_ref(), &deps.install_id)
                        .map_err(map_keys_error)?;
                Self::open_with_ark(deps, ark, None, None)
                    .map(Opened::Ready)
                    .map_err(|(_, err)| err)
            }
            ArkState::Unlocked(ark) => Self::open_with_ark(deps, ark, None, None)
                .map(Opened::Ready)
                .map_err(|(_, err)| err),
            ArkState::Locked(locked) => Ok(Opened::Locked(LockedCore { deps, locked })),
        }
    }

    /// Locks the workspace: closes the database and zeroises key material. The app
    /// lock must be enabled (`InvalidInput` otherwise, the core is returned).
    // The caller gets its core back on failure; this runs once per lock.
    #[allow(clippy::result_large_err)]
    pub fn lock(self) -> Result<LockedCore, (Core, ApiError)> {
        if self.security.lock.is_none() {
            return Err((
                self,
                ApiError::invalid("lock", "the app lock is not enabled"),
            ));
        }
        let state = match lk::load_ark_state(self.keystore.as_ref(), &self.install_id) {
            Ok(state) => state,
            Err(err) => return Err((self, map_keys_error(err))),
        };
        let ArkState::Locked(locked) = state else {
            return Err((
                self,
                ApiError::internal("the keystore no longer holds a locked root key"),
            ));
        };
        tracing::info!("workspace locked");
        Ok(LockedCore {
            deps: self.into_deps(),
            locked,
        })
    }

    /// The whole open sequence once the ARK is known. On failure the dependencies are
    /// handed back so a locked core can stay usable.
    #[allow(clippy::result_large_err)]
    pub(crate) fn open_with_ark(
        deps: CoreDeps,
        ark: Ark,
        lock: Option<Argon2Params>,
        passphrase: Option<&str>,
    ) -> Result<Core, (CoreDeps, ApiError)> {
        let prepared = match Self::prepare(&deps, ark, lock, passphrase) {
            Ok(prepared) => prepared,
            Err(err) => return Err((deps, err)),
        };
        let CoreDeps {
            install_id,
            platform,
            data_dir,
            keystore,
            opener,
            clock,
            rng,
            lock_params,
            blob_store,
        } = deps;
        let mut core = Core {
            db: prepared.db,
            clock,
            rng,
            keystore,
            ark: prepared.ark,
            install_id,
            opener,
            platform,
            data_dir,
            device: prepared.device,
            docs: prepared.docs,
            policy: crate::CompactionPolicy::default(),
            events: Vec::new(),
            security: prepared.security,
            lock_params,
            blob_store,
            uploads: std::collections::HashMap::new(),
            sync: crate::sync::SyncRuntime::default(),
            pairing: None,
            recovery: None,
            export: None,
            export_counter: 0,
            import: None,
            import_counter: 0,
            ai: crate::ai::AiState::default(),
            shared: crate::membership::SharedRuntime::default(),
            invite: None,
        };
        match core.finish_open() {
            Ok(()) => Ok(core),
            Err(err) => Err((core.into_deps(), err)),
        }
    }

    /// Gives the platform pieces back (closing the database and dropping the ARK).
    fn into_deps(self) -> CoreDeps {
        let Core {
            db,
            clock,
            rng,
            keystore,
            ark,
            install_id,
            opener,
            platform,
            data_dir,
            lock_params,
            blob_store,
            ..
        } = self;
        drop(db);
        drop(ark);
        CoreDeps {
            install_id,
            platform,
            data_dir,
            keystore,
            opener,
            clock,
            rng,
            lock_params,
            blob_store,
        }
    }

    /// Keystore, database, device identity, documents and the security state.
    fn prepare(
        deps: &CoreDeps,
        ark: Ark,
        lock: Option<Argon2Params>,
        passphrase: Option<&str>,
    ) -> Result<Prepared, ApiError> {
        let keystore = deps.keystore.as_ref();
        let rng = deps.rng.as_ref();
        let install_id = &deps.install_id;

        let device_keys = kb_keys::load_or_create_device_keys(keystore, rng, install_id)
            .map_err(map_keys_error)?;

        let (ark, mut db) = Self::open_database(deps, ark, lock, passphrase)?;

        let device = match db.device_get().map_err(map_storage_error)? {
            Some(row) => row,
            None => {
                let row = kb_storage::DeviceRow {
                    device_id: DeviceId::generate(rng),
                    name: crate::default_device_name(&deps.platform),
                    created_at: deps.clock.now_ms(),
                };
                db.device_init(&row, kb_types::CORE_VERSION)
                    .map_err(map_storage_error)?;
                tracing::info!(device_id = %short_id(&row.device_id), "initialised device identity");
                row
            }
        };

        let docs = crate::documents::load_documents(&db, rng)?;
        crate::documents::verify_index(&mut db, &docs)?;
        Core::gc_page_documents(&mut db, &docs)?;

        let security = Self::load_security(&db, &ark, device_keys, device.device_id, lock)?;
        Ok(Prepared {
            ark,
            db,
            device,
            docs,
            security,
        })
    }

    /// Opens the database with the ARK, finishing an interrupted rotation when the
    /// keystore still parks the next root key.
    fn open_database(
        deps: &CoreDeps,
        ark: Ark,
        lock: Option<Argon2Params>,
        passphrase: Option<&str>,
    ) -> Result<(Ark, Db), ApiError> {
        let keystore = deps.keystore.as_ref();
        let install_id = &deps.install_id;
        let db_key = kb_keys::derive_db_key(&ark, install_id);
        match deps.opener.open(&db_key) {
            Ok(opened) => {
                // A parked next key that the database does not need is a leftover of
                // a rotation that never rekeyed; the next step of that rotation is
                // decided in `finish_open` from the `rotation_pending` flag.
                let db = Db::new(opened).map_err(map_storage_error)?;
                Ok((ark, db))
            }
            Err(StorageError::WrongKey) => {
                let Some(next) =
                    Self::parked_next_ark(keystore, install_id, passphrase, &deps.lock_params)?
                else {
                    return Err(map_storage_error(StorageError::WrongKey));
                };
                let next_key = kb_keys::derive_db_key(&next, install_id);
                let opened = deps.opener.open(&next_key).map_err(map_storage_error)?;
                let db = Db::new(opened).map_err(map_storage_error)?;
                // The rotation rekeyed the database but did not promote the keystore.
                lk::store_ark(
                    keystore,
                    install_id,
                    &next,
                    passphrase.zip(lock),
                    deps.rng.as_ref(),
                )
                .map_err(map_keys_error)?;
                Self::clear_parked_next(keystore, install_id);
                tracing::warn!("finished an interrupted root key rotation at open");
                Ok((next, db))
            }
            Err(err) => Err(map_storage_error(err)),
        }
    }

    /// The parked next ARK, if a rotation left one.
    fn parked_next_ark(
        keystore: &dyn Keystore,
        install_id: &InstallId,
        passphrase: Option<&str>,
        minimum: &Argon2Params,
    ) -> Result<Option<Ark>, ApiError> {
        if let Some(bytes) = keystore
            .get(&next_entry_name(install_id))
            .map_err(|e| map_keys_error(KeysError::Keystore(e)))?
        {
            let bytes: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| ApiError::crypto("the parked root key has the wrong length"))?;
            return Ok(Some(Ark::from_secret(Secret32::from_bytes(bytes))));
        }
        if let Some(bytes) = keystore
            .get(&next_locked_entry_name(install_id))
            .map_err(|e| map_keys_error(KeysError::Keystore(e)))?
        {
            let Some(passphrase) = passphrase else {
                return Err(ApiError::locked());
            };
            let blob = kb_crypto::PassphraseWrapped::from_bytes(&bytes)
                .map_err(|e| map_keys_error(KeysError::Crypto(e)))?;
            let secret = kb_crypto::unwrap_with_passphrase(
                &blob,
                passphrase.as_bytes(),
                &next_locked_subject(install_id),
                minimum,
            )
            .map_err(|e| map_keys_error(KeysError::Crypto(e)))?;
            return Ok(Some(Ark::from_secret(secret)));
        }
        Ok(None)
    }

    fn clear_parked_next(keystore: &dyn Keystore, install_id: &InstallId) {
        let _ = keystore.delete(&next_entry_name(install_id));
        let _ = keystore.delete(&next_locked_entry_name(install_id));
    }

    /// Reads the device log, the keyring and the recovery meta.
    pub(crate) fn load_security(
        db: &Db,
        ark: &Ark,
        device_keys: DeviceKeys,
        this_device: DeviceId,
        lock: Option<Argon2Params>,
    ) -> Result<Security, ApiError> {
        let rows = db.device_log_all().map_err(map_storage_error)?;
        let entries = rows
            .iter()
            .map(|row| LogEntry::decode(&row.entry))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;
        let (log, log_state) = if entries.is_empty() {
            (None, None)
        } else {
            let log =
                DeviceLog::from_entries(entries).map_err(|e| map_keys_error(KeysError::Log(e)))?;
            let state = log
                .verify()
                .map_err(|e| map_keys_error(KeysError::Log(e)))?;
            (Some(log), Some(state))
        };

        // Items were verified when they entered (created here, received through sync
        // or restored from a backup) and the database is encrypted and authenticated
        // page by page, so the open only checks that every signer is a known device;
        // re-verifying every signature at each open costs seconds on large keyrings.
        // The items of a shared space are signed by the devices of the other members too (they
        // were checked against the member's verified device log when they came in, which is not
        // loaded yet here): for a shared space any signer was accepted then, and still is.
        let shared_spaces: HashSet<SpaceId> = db
            .sync_feed_list()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|feed| feed.kind == FeedKind::Shared)
            .filter_map(|feed| feed.space_id)
            .collect();
        let mut keyring = Keyring::new();
        for row in db.keyring_all().map_err(map_storage_error)? {
            let item = KeyringItem::decode(&row.record).map_err(map_keys_error)?;
            let created_by = match &item {
                KeyringItem::Record(r) => r.created_by,
                KeyringItem::Tombstone(t) => t.purged_by,
            };
            let known = created_by == this_device
                || shared_spaces.contains(&row.space_id)
                || log_state
                    .as_ref()
                    .is_some_and(|s| s.devices.iter().any(|d| d.device_id == created_by));
            if !known {
                return Err(ApiError::crypto(
                    "a keyring item is signed by an unknown device",
                ));
            }
            keyring.merge(item);
        }

        let recovery = match (
            db.meta_get(META_RECOVERY_ID).map_err(map_storage_error)?,
            db.meta_get(META_RECOVERY_BLOB).map_err(map_storage_error)?,
        ) {
            (Some(id), Some(blob)) => {
                let id = decode_hex(&id)
                    .and_then(|v| <[u8; 32]>::try_from(v).ok())
                    .ok_or_else(|| ApiError::storage("the stored recovery id is malformed"))?;
                let blob = decode_hex(&blob)
                    .ok_or_else(|| ApiError::storage("the stored recovery blob is malformed"))?;
                let blob = WrappedKey::from_bytes(&blob)
                    .map_err(|e| map_keys_error(KeysError::Crypto(e)))?;
                Some(RecoveryMeta {
                    id,
                    blob,
                    created_at: meta_i64(db, META_RECOVERY_CREATED)?.unwrap_or_default(),
                    verified_at: meta_i64(db, META_RECOVERY_VERIFIED)?,
                    reissued_at: meta_i64(db, META_RECOVERY_REISSUED)?,
                })
            }
            _ => None,
        };
        let ark_generation = meta_i64(db, META_ARK_GENERATION)?
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);
        let recovery_stale = db
            .meta_get(META_RECOVERY_STALE)
            .map_err(map_storage_error)?
            .is_some_and(|v| v == "1");
        let previous_arks = db
            .meta_get(META_PREVIOUS_ARKS)
            .map_err(map_storage_error)?
            .map(|v| parse_previous_arks(&v))
            .unwrap_or_default();
        let pending_rotation = db
            .meta_get(META_PENDING_ROTATION)
            .map_err(map_storage_error)?
            .and_then(|v| {
                let (generation, sealed) = v.split_once(':')?;
                Some((generation.parse().ok()?, decode_hex(sealed)?))
            });
        let revoked = db
            .meta_get(META_REVOKED_AT)
            .map_err(map_storage_error)?
            .is_some_and(|v| !v.is_empty());
        let recovery_published = db
            .meta_get(META_RECOVERY_PUBLISHED)
            .map_err(map_storage_error)?
            .is_some_and(|v| v == "1");
        let recovery_obsolete = db
            .meta_get(META_RECOVERY_OBSOLETE)
            .map_err(map_storage_error)?
            .map(|v| parse_ids(&v))
            .unwrap_or_default();

        let _ = ark;
        Ok(Security {
            device_keys,
            log,
            log_state,
            keyring,
            space_keys: HashMap::new(),
            previous_arks,
            pending_rotation,
            revoked,
            recovery_published,
            recovery_obsolete,
            recovery,
            lock,
            ark_generation,
            recovery_stale,
        })
    }

    /// The steps that need the assembled core: a workspace from Milestone 1 gets its
    /// account document, derived indexes are checked, an interrupted rotation is
    /// completed, every space and document gets its key record.
    fn finish_open(&mut self) -> Result<(), ApiError> {
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some()
        {
            self.ensure_account()?;
        }
        // Moves between spaces (ADR-0038): pages another device left in two spaces, orphans below
        // residue, the space of the page documents; before the derived rows are compared.
        self.reconcile_moves_at_open()?;
        self.verify_doc_spaces()?;
        self.verify_text_index()?;
        self.finish_pending_rotation()?;
        self.keyring_maintain()?;
        self.verify_attachment_index()?;
        let now = self.clock.now_ms();
        self.attachment_gc(now)?;
        self.sync_load()?;
        // The membership of the shared spaces (timelines, the co-members' device logs).
        if let Err(error) = self.shared_load() {
            tracing::warn!(code = ?error.code, "the shared spaces' membership could not be loaded");
        }
        // After the sync state is known, so that the purges ask the driver for a cycle (ADR-0040).
        self.trash_expire_at_open();
        Ok(())
    }

    /// A rotation that wrote its rows but never got to the final step.
    fn finish_pending_rotation(&mut self) -> Result<(), ApiError> {
        let pending = self
            .db
            .meta_get(META_ROTATION_PENDING)
            .map_err(map_storage_error)?
            .is_some_and(|v| v == "1");
        let parked_plain = self
            .keystore
            .get(&next_entry_name(&self.install_id))
            .map_err(|e| map_keys_error(KeysError::Keystore(e)))?;
        let parked_locked = self
            .keystore
            .get(&next_locked_entry_name(&self.install_id))
            .map_err(|e| map_keys_error(KeysError::Keystore(e)))?;
        if !pending {
            if parked_plain.is_some() || parked_locked.is_some() {
                // Parked before any row was written: nothing depends on it.
                Self::clear_parked_next(self.keystore.as_ref(), &self.install_id);
            }
            return Ok(());
        }
        // Rows were written for the parked key. If the database opened with the
        // current ARK, the rekey never happened: do it now with the parked key.
        if let Some(bytes) = parked_plain {
            let bytes: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| ApiError::crypto("the parked root key has the wrong length"))?;
            let next = Ark::from_secret(Secret32::from_bytes(bytes));
            self.switch_ark(next, None)?;
        } else if parked_locked.is_some() {
            // The locked parked key needs the passphrase, which `open_database` had
            // when the rekey had happened; here it had not, and the passphrase is gone.
            // Leave the flag: the next unlock gets another chance.
            tracing::warn!(
                "a locked root key rotation is still pending; unlock again to finish it"
            );
            return Ok(());
        }
        // The new recovery blob was never written: the stored kit is stale.
        self.db
            .transaction(|tx| {
                tx.meta_set(META_RECOVERY_STALE, "1")?;
                tx.meta_set(META_ROTATION_PENDING, "0")
            })
            .map_err(map_storage_error)?;
        self.security.recovery_stale = true;
        Self::clear_parked_next(self.keystore.as_ref(), &self.install_id);
        tracing::warn!(
            "a root key rotation was interrupted before the new recovery kit was stored"
        );
        Ok(())
    }

    /// Rekeys the database and promotes the keystore to `next`; the in-memory ARK
    /// follows. `lock` carries the passphrase when the app lock is on.
    pub(crate) fn switch_ark(&mut self, next: Ark, lock: Option<&str>) -> Result<(), ApiError> {
        let key = kb_keys::derive_db_key(&next, &self.install_id);
        if let Err(err) = self.db.rekey(&key) {
            // A failed rekey must not be written through (SQLCipher can report success
            // while the file still has the old key). Reopen with the old key; if that
            // key is refused, the rekey did land and the new key is current.
            let old_key = kb_keys::derive_db_key(&self.ark, &self.install_id);
            match self.opener.open(&old_key) {
                Ok(opened) => {
                    self.db = Db::new(opened).map_err(map_storage_error)?;
                    return Err(map_storage_error(err));
                }
                Err(StorageError::WrongKey) => {
                    let opened = self.opener.open(&key).map_err(map_storage_error)?;
                    self.db = Db::new(opened).map_err(map_storage_error)?;
                    tracing::warn!("the database rekey reported an error but did take effect");
                }
                Err(other) => return Err(map_storage_error(other)),
            }
        }
        lk::store_ark(
            self.keystore.as_ref(),
            &self.install_id,
            &next,
            lock.zip(self.security.lock),
            self.rng.as_ref(),
        )
        .map_err(map_keys_error)?;
        Self::clear_parked_next(self.keystore.as_ref(), &self.install_id);
        self.ark = next;
        Ok(())
    }

    // ----- keyring maintenance ----------------------------------------------------

    /// Every space has a `Space(0)` record and every document a `Doc` record.
    pub(crate) fn keyring_maintain(&mut self) -> Result<(), ApiError> {
        // A shared space's keys come from its feed (M13): this device never invents them.
        let shared = self.shared_space_map()?;
        let spaces: Vec<(SpaceId, DocId)> = self
            .docs
            .structures
            .iter()
            .filter(|(space, _)| !shared.contains_key(*space))
            .map(|(space, open)| (*space, open.doc_id))
            .collect();
        for (space_id, doc_id) in spaces {
            self.ensure_space_key(space_id)?;
            self.ensure_doc_key(space_id, doc_id)?;
        }
        let pages = self
            .db
            .docs_by_kind(DocKindCode::Page)
            .map_err(map_storage_error)?;
        for row in pages {
            if let Some(space_id) = row.space_id
                && self.docs.structures.contains_key(&space_id)
                && !shared.contains_key(&space_id)
            {
                self.ensure_doc_key(space_id, row.doc_id)?;
            }
        }
        Ok(())
    }

    /// The current key epoch of a space: the highest `Space` record's; 0 when the
    /// space has no record yet. Epochs bump on revocation (ADR-0022).
    pub(crate) fn space_epoch(&self, space_id: SpaceId) -> u32 {
        self.security
            .keyring
            .space_records(space_id)
            .iter()
            .filter_map(|r| match r.subject {
                KeySubject::Space { epoch } => Some(epoch),
                _ => None,
            })
            .max()
            .unwrap_or(SPACE_EPOCH)
    }

    /// Unwraps a space record under the current root key, or a retained previous one
    /// (records wrapped before a rotation this device has not re-wrapped yet).
    fn unwrap_space_record(&self, record: &kb_keys::KeyRecord) -> Result<Secret32, ApiError> {
        match kr::unwrap_space_key(&self.ark, record) {
            Ok(key) => Ok(key),
            Err(err) => {
                for (_, ark) in &self.security.previous_arks {
                    if let Ok(key) = kr::unwrap_space_key(ark, record) {
                        return Ok(key);
                    }
                }
                tracing::error!(space = %short_id(&record.space_id), "the space key record does not unwrap under any root key this device holds");
                Err(map_keys_error(err))
            }
        }
    }

    /// The unwrapped space key of an epoch; `None` when the keyring has no record for
    /// it.
    pub(crate) fn space_key_at(
        &mut self,
        space_id: SpaceId,
        epoch: u32,
    ) -> Result<Option<Secret32>, ApiError> {
        if let Some(key) = self.security.space_keys.get(&(space_id, epoch)) {
            return Ok(Some(key.clone()));
        }
        let Some(record) = self
            .security
            .keyring
            .record(space_id, &KeySubject::Space { epoch })
            .cloned()
        else {
            return Ok(None);
        };
        let key = self.unwrap_space_record(&record)?;
        self.security
            .space_keys
            .insert((space_id, epoch), key.clone());
        Ok(Some(key))
    }

    /// The space key of the current epoch, creating the first record when the space
    /// has none.
    pub(crate) fn ensure_space_key(&mut self, space_id: SpaceId) -> Result<Secret32, ApiError> {
        let epoch = self.space_epoch(space_id);
        if let Some(key) = self.space_key_at(space_id, epoch)? {
            return Ok(key);
        }
        if self.shared_feed_of(space_id)?.is_some() {
            // The key of a shared space is the feed's (ADR-0044): not this device's to make.
            return Err(ApiError::crypto(
                "this device does not hold the space key of this shared space",
            ));
        }
        let now = self.clock.now_ms();
        let (key, record) = kr::new_space_key(
            &self.ark,
            space_id,
            epoch,
            &self.security.device_keys,
            self.device.device_id,
            now,
            self.rng.as_ref(),
        );
        self.put_keyring_item(KeyringItem::Record(record))?;
        self.security
            .space_keys
            .insert((space_id, epoch), key.clone());
        Ok(key)
    }

    /// Whether a document's key is tombstoned at any epoch (purged).
    fn doc_is_tombstoned(&self, space_id: SpaceId, doc_id: DocId) -> bool {
        (0..=self.space_epoch(space_id)).any(|epoch| {
            self.security
                .keyring
                .is_tombstoned(space_id, &KeySubject::Doc { doc_id, epoch })
        })
    }

    /// Creates the `Doc` record of a document at the space's current epoch when it has
    /// none (and is not purged).
    pub(crate) fn ensure_doc_key(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
    ) -> Result<(), ApiError> {
        match self.doc_key_item(space_id, doc_id)? {
            Some(item) => self.put_keyring_item(item),
            None => Ok(()),
        }
    }

    /// The `Doc` record [`Self::ensure_doc_key`] would store, made but not stored (`None` when
    /// the document has its record or is purged). A caller that creates many documents stores
    /// the records in the transaction that writes them and merges them into the in-memory
    /// keyring afterwards (`Core::keyring_merge`).
    ///
    /// A document that already has a record in **another** space and none in this one is a page
    /// another device moved here (ADR-0038): only the device that performs a move writes the
    /// target's record, so nobody else creates one (`None`; the source's record serves until the
    /// moving device's arrives).
    pub(crate) fn doc_key_item(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
    ) -> Result<Option<KeyringItem>, ApiError> {
        if self.doc_record_is_elsewhere(space_id, doc_id) {
            return Ok(None);
        }
        self.new_doc_key_item(space_id, doc_id, MemberRole::Editor)
    }

    /// [`Self::ensure_doc_key`] for the `Doc` record of a comment stream, which a Commenter may
    /// make too (ADR-0048: the stream's id is the same for everybody, so two Commenters make the
    /// record at once and the first one in the feed wins).
    pub(crate) fn ensure_comment_stream_key(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
    ) -> Result<(), ApiError> {
        if self.doc_record_is_elsewhere(space_id, doc_id) {
            return Ok(());
        }
        match self.new_doc_key_item(space_id, doc_id, MemberRole::Commenter)? {
            Some(item) => self.put_keyring_item(item),
            None => Ok(()),
        }
    }

    /// The `Doc` record a document needs before it is sent (a comment stream: the Commenter's
    /// rule, any other document: the Editor's).
    pub(crate) fn ensure_doc_key_of(&mut self, row: &kb_storage::DocRow) -> Result<(), ApiError> {
        let Some(space_id) = row.space_id else {
            return Ok(());
        };
        if row.kind == DocKindCode::Comments {
            self.ensure_comment_stream_key(space_id, row.doc_id)
        } else {
            self.ensure_doc_key(space_id, row.doc_id)
        }
    }

    /// The `Doc` record this device writes when it moves the document into `space_id`
    /// (ADR-0038): like [`Self::doc_key_item`] without the rule that keeps other devices from
    /// creating it. `None` when the space already has the record at its current epoch (a page
    /// moving back) or the document is purged there.
    pub(crate) fn doc_key_item_for_move(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
    ) -> Result<Option<KeyringItem>, ApiError> {
        self.new_doc_key_item(space_id, doc_id, MemberRole::Editor)
    }

    /// Whether the keyring holds a `Doc` record of the document in another space and none in
    /// `space_id` (at any epoch).
    pub(crate) fn doc_record_is_elsewhere(&self, space_id: SpaceId, doc_id: DocId) -> bool {
        let spaces = self.security.keyring.doc_spaces(doc_id);
        !spaces.is_empty() && !spaces.contains(&space_id)
    }

    fn new_doc_key_item(
        &mut self,
        space_id: SpaceId,
        doc_id: DocId,
        required: MemberRole,
    ) -> Result<Option<KeyringItem>, ApiError> {
        // A hostile or out-of-date device of the tests makes its keys whatever the role says.
        #[cfg(test)]
        let required = if self.shared.force_push {
            MemberRole::Viewer
        } else {
            required
        };
        let epoch = self.space_epoch(space_id);
        let subject = KeySubject::Doc { doc_id, epoch };
        if self.doc_is_tombstoned(space_id, doc_id)
            || self.security.keyring.record(space_id, &subject).is_some()
        {
            return Ok(None);
        }
        if let Some(row) = self.shared_feed_of(space_id)? {
            // In a shared space only an Editor or an Owner makes records (a Commenter only those of
            // a comment stream), under the key of the feed's epoch; an archive makes none.
            if self.feed_archived(row.feed)?.is_some()
                || self.space_key_at(space_id, epoch)?.is_none()
                || !self
                    .role_in_space(space_id)?
                    .is_some_and(|role| role >= required)
            {
                return Ok(None);
            }
        }
        let space_key = self.ensure_space_key(space_id)?;
        let now = self.clock.now_ms();
        let (_key, record) = kr::new_doc_key(
            &space_key,
            space_id,
            doc_id,
            epoch,
            &self.security.device_keys,
            self.device.device_id,
            now,
            self.rng.as_ref(),
        );
        Ok(Some(KeyringItem::Record(record)))
    }

    /// Merges records that were stored by the caller's own transaction into the in-memory
    /// keyring (what [`Self::put_keyring_item`] does after its write).
    pub(crate) fn keyring_merge(&mut self, items: impl IntoIterator<Item = KeyringItem>) {
        for item in items {
            self.security.keyring.merge(item);
        }
        self.sync_note_local_change();
    }

    /// Tombstones the documents of purged pages (crypto-erase): every epoch that has
    /// a record, and the current one, in `space_id` and in every other space that holds a
    /// record of the document (a page that moved between spaces, ADR-0038, has one in each
    /// space it lived in).
    pub(crate) fn keyring_tombstone_docs(
        &mut self,
        space_id: SpaceId,
        docs: &[DocId],
    ) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        for doc_id in docs {
            let mut spaces = self.security.keyring.doc_spaces(*doc_id);
            if !spaces.contains(&space_id) {
                spaces.push(space_id);
            }
            for space in spaces {
                let current = self.space_epoch(space);
                for epoch in 0..=current {
                    let subject = KeySubject::Doc {
                        doc_id: *doc_id,
                        epoch,
                    };
                    let has_record = self.security.keyring.record(space, &subject).is_some();
                    if self.security.keyring.is_tombstoned(space, &subject)
                        || (!has_record && epoch != current)
                    {
                        continue;
                    }
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
        }
        Ok(())
    }

    /// Forgets a deleted space's keys.
    pub(crate) fn keyring_forget_space(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        self.db
            .keyring_delete_space(space_id)
            .map_err(map_storage_error)?;
        self.security.keyring.remove_space(space_id);
        self.security
            .space_keys
            .retain(|(space, _), _| *space != space_id);
        Ok(())
    }

    pub(crate) fn put_keyring_item(&mut self, item: KeyringItem) -> Result<(), ApiError> {
        let row = keyring_row(&item);
        // Stored only when it is what the keyring holds afterwards (an item that loses against
        // what is there must not replace the stored row).
        if self.merge_keyring_item(item)? {
            self.db.keyring_put(&row).map_err(map_storage_error)?;
        }
        self.sync_note_local_change();
        Ok(())
    }

    /// Merges one item into the in-memory keyring; answers whether the keyring changed (the item
    /// is now what it holds for its subject).
    ///
    /// A `Doc` record this device created that another record replaces (two devices made the
    /// record of one document at the same time: both moved the page into the same space,
    /// ADR-0038, or both ran `keyring_maintain` for it) leaves entries this device sealed under a
    /// key nobody else has. The document is marked for a snapshot: the next cycle publishes its
    /// whole state sealed under the winning record, which covers every entry this device pushed
    /// (its cursor has passed them), so peers apply the snapshot and drop the items they parked
    /// because they could not open those entries.
    pub(crate) fn merge_keyring_item(&mut self, item: KeyringItem) -> Result<bool, ApiError> {
        let displaced = match &item {
            KeyringItem::Record(record) => match record.subject {
                KeySubject::Doc { doc_id, .. } => self
                    .security
                    .keyring
                    .record(record.space_id, &record.subject)
                    .filter(|own| own.created_by == self.device.device_id && **own != *record)
                    .map(|_| doc_id),
                _ => None,
            },
            KeyringItem::Tombstone(_) => None,
        };
        let changed = self.security.keyring.merge(item);
        if changed && let Some(doc_id) = displaced {
            tracing::info!(doc = %short_id(&doc_id), "this device's document key lost to another record; snapshot scheduled");
            self.sync_mark_for_snapshot(&[doc_id])?;
        }
        Ok(changed)
    }

    /// Merges one item of a **shared** feed's keyring in feed order (`Keyring::merge_ordered`, ADR-0044):
    /// the first record of a subject wins and a tombstone counts only when `is_owner_at` says so. A
    /// record this device made and has not sent yet is not in the feed, so a record of the feed for
    /// the same subject came first: it replaces the local one, so does a record this device sent in an
    /// entry that comes later in the feed than the item (`cursor`), and the document is marked for a
    /// snapshot sealed under the winner (the entries this device made under its own key are unreadable
    /// to the others). Answers whether the keyring changed.
    pub(crate) fn merge_keyring_item_ordered(
        &mut self,
        feed: FeedId,
        cursor: u64,
        item: KeyringItem,
        is_owner_at: &dyn Fn(&kb_keys::Tombstone) -> bool,
    ) -> Result<bool, ApiError> {
        let mut displaced: Option<DocId> = None;
        if let KeyringItem::Record(record) = &item
            && let Some(own) = self
                .security
                .keyring
                .record(record.space_id, &record.subject)
                .filter(|own| own.created_by == self.device.device_id && **own != *record)
                .cloned()
        {
            let subject = own.subject.encode();
            let unsent = self
                .db
                .keyring_unpublished()
                .map_err(map_storage_error)?
                .iter()
                .any(|row| row.space_id == own.space_id && row.subject == subject);
            // Or sent, in an entry of the feed that comes after this item: not in the feed yet from
            // where the pull stands.
            let later = self.own_record_pushed_after(feed, own.space_id, &subject, cursor)?;
            if unsent || later {
                let kept: Vec<KeyringItem> = self
                    .security
                    .keyring
                    .items()
                    .into_iter()
                    .filter(|other| {
                        !(other.space_id() == own.space_id && other.subject() == own.subject)
                    })
                    .collect();
                let mut fresh = Keyring::new();
                for other in kept {
                    fresh.merge(other);
                }
                self.security.keyring = fresh;
                if let KeySubject::Doc { doc_id, .. } = own.subject {
                    displaced = Some(doc_id);
                }
            }
        }
        let changed = self.security.keyring.merge_ordered(item, is_owner_at);
        if changed && let Some(doc_id) = displaced {
            tracing::info!(doc = %short_id(&doc_id), "this device's document key lost to the feed's record; snapshot scheduled");
            self.feed_mark_for_snapshot(feed, doc_id)?;
        }
        Ok(changed)
    }

    // ----- requests ---------------------------------------------------------------

    /// Dispatch entry for the Milestone 4 requests.
    pub(crate) fn dispatch_security(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::SecurityStatus => Ok(Response::Security(self.security_info())),
            Request::RecoveryKitCreate => Ok(Response::RecoveryKit(self.recovery_kit_create()?)),
            Request::RecoveryKitVerify { kit } => {
                self.recovery_kit_verify(&kit)?;
                Ok(Response::Ok)
            }
            Request::RecoveryKitReissue { passphrase } => Ok(Response::RecoveryKit(
                self.recovery_kit_reissue(passphrase.as_deref())?,
            )),
            Request::AppLockEnable { passphrase } => {
                self.app_lock_enable(&passphrase)?;
                Ok(Response::Ok)
            }
            Request::AppLockDisable { passphrase } => {
                self.app_lock_disable(&passphrase)?;
                Ok(Response::Ok)
            }
            Request::AppLockChange { current, new } => {
                self.app_lock_change(&current, &new)?;
                Ok(Response::Ok)
            }
            Request::DeviceList => Ok(Response::Devices(self.device_list()?)),
            other => Err(ApiError::internal(format!(
                "request {} is not a security request",
                other.kind()
            ))),
        }
    }

    pub(crate) fn security_info(&self) -> SecurityInfo {
        let counts = self.security.keyring.counts();
        let params = self.security.lock.unwrap_or(self.lock_params);
        SecurityInfo {
            account_id: self.security.log_state.as_ref().map(|s| s.account_id),
            recovery_kit: self.security.recovery.as_ref().map(|r| RecoveryKitInfo {
                created_at: r.created_at,
                verified_at: r.verified_at,
                reissued_at: r.reissued_at,
                stale: self.security.recovery_stale,
            }),
            app_lock: AppLockInfo {
                enabled: self.security.lock.is_some(),
                memory_kib: params.memory_kib,
                iterations: params.iterations,
                parallelism: params.parallelism,
            },
            this_device: self.device.device_id,
            devices: self
                .security
                .log_state
                .as_ref()
                .map_or(0, |s| u32::try_from(s.devices.len()).unwrap_or(u32::MAX)),
            keyring: KeyringInfo {
                spaces: counts.spaces,
                documents: counts.documents,
                attachments: counts.attachments,
                tombstones: counts.tombstones,
            },
            root_generation: self.security.ark_generation,
            rotation_pending: self.security.pending_rotation.is_some(),
            this_device_revoked: self.security.revoked,
            recovery_published: self.security.recovery_published,
        }
    }

    /// Whether the stored kit does not open the current root key (see
    /// [`Security::recovery_stale`]); exposed for diagnostics and the UI banner.
    #[must_use]
    pub fn recovery_kit_is_stale(&self) -> bool {
        self.security.recovery_stale
    }

    fn recovery_kit_create(&mut self) -> Result<RecoveryKit, ApiError> {
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_none()
        {
            return Err(ApiError::invalid("workspace", "create the workspace first"));
        }
        if self.security.recovery.is_some() && !self.security.recovery_stale {
            return Err(ApiError::already_exists(
                "a recovery kit already exists; re-issue it instead",
            ));
        }
        let now = self.clock.now_ms();
        let rck = RecoveryKey::generate(self.rng.as_ref());
        let recovery_key = rck.signing_key().verifying_key();
        let blob = rck.wrap_ark(&self.ark, self.rng.as_ref());
        let id = rck.recovery_id();

        // The device log starts with the kit (Genesis needs the recovery key); an
        // existing log records the new key instead.
        let new_entries: Vec<LogEntry> = match self.security.log.as_mut() {
            None => {
                let name = lg::encrypt_name(
                    &self.ark,
                    self.device.device_id,
                    &self.device.name,
                    self.rng.as_ref(),
                );
                let log = DeviceLog::genesis(
                    self.device.device_id,
                    &self.security.device_keys,
                    recovery_key,
                    name,
                    now,
                )
                .map_err(|e| map_keys_error(KeysError::Log(e)))?;
                let entries = log.entries().to_vec();
                let state = log
                    .verify()
                    .map_err(|e| map_keys_error(KeysError::Log(e)))?;
                self.security.log = Some(log);
                self.security.log_state = Some(state);
                entries
            }
            Some(log) => {
                let entry = log
                    .rotate_recovery(
                        recovery_key,
                        (&self.security.device_keys, self.device.device_id),
                        now,
                    )
                    .map_err(|e| map_keys_error(KeysError::Log(e)))?
                    .clone();
                self.security.log_state = Some(
                    log.verify()
                        .map_err(|e| map_keys_error(KeysError::Log(e)))?,
                );
                vec![entry]
            }
        };
        let account_id = self
            .security
            .log_state
            .as_ref()
            .map(|s| s.account_id)
            .ok_or_else(|| ApiError::internal("device log without state"))?;
        let created_at = self
            .security
            .recovery
            .as_ref()
            .map_or(now, |r| r.created_at);
        let reissued = self.security.recovery.is_some();
        self.db
            .transaction(|tx| {
                for entry in &new_entries {
                    tx.device_log_append(&DeviceLogRow {
                        seq: entry.seq,
                        entry: entry.encode(),
                        hash: entry.hash(),
                    })?;
                }
                tx.meta_set(META_ACCOUNT_ID, &account_id.to_string())?;
                tx.meta_set(META_RECOVERY_ID, &hex(&id))?;
                tx.meta_set(META_RECOVERY_BLOB, &hex(&blob.to_bytes()))?;
                tx.meta_set(META_RECOVERY_CREATED, &created_at.to_string())?;
                tx.meta_set(META_RECOVERY_VERIFIED, "")?;
                if reissued {
                    tx.meta_set(META_RECOVERY_REISSUED, &now.to_string())?;
                }
                tx.meta_set(META_RECOVERY_STALE, "0")?;
                tx.meta_set(META_RECOVERY_PUBLISHED, "0")?;
                Ok(())
            })
            .map_err(map_storage_error)?;
        self.security.recovery = Some(RecoveryMeta {
            id,
            blob,
            created_at,
            verified_at: None,
            reissued_at: reissued.then_some(now),
        });
        self.security.recovery_stale = false;
        self.security.recovery_published = false;
        self.events.push(Event::SecurityChanged);
        self.sync_note_local_change();
        tracing::info!("recovery kit created");
        Ok(RecoveryKit {
            words: rck.words(),
            code: rck.code(),
            account_id,
            created_at: now,
        })
    }

    fn recovery_kit_verify(&mut self, kit: &str) -> Result<(), ApiError> {
        let Some(meta) = self.security.recovery.as_ref() else {
            return Err(ApiError::not_found("no recovery kit exists yet"));
        };
        let typed = RecoveryKey::from_kit(kit.trim()).map_err(map_keys_error)?;
        if typed.recovery_id() != meta.id {
            return Err(ApiError::invalid(
                "kit",
                "the kit does not match this workspace",
            ));
        }
        let now = self.clock.now_ms();
        self.db
            .meta_set(META_RECOVERY_VERIFIED, &now.to_string())
            .map_err(map_storage_error)?;
        if let Some(meta) = self.security.recovery.as_mut() {
            meta.verified_at = Some(now);
        }
        self.events.push(Event::SecurityChanged);
        Ok(())
    }

    /// New kit, new root key: ADR-0017.
    fn recovery_kit_reissue(&mut self, passphrase: Option<&str>) -> Result<RecoveryKit, ApiError> {
        self.rotate_root(passphrase, None)
    }

    /// Rotates the root key (ADR-0017) and, since Milestone 7, bumps every space's key
    /// epoch and tells the other devices through a `RootRotated` entry (ADR-0022);
    /// `revoke` names a device to revoke first, so that it gets no box. Issues the new
    /// recovery kit. See the module docs for the crash protocol.
    pub(crate) fn rotate_root(
        &mut self,
        passphrase: Option<&str>,
        revoke: Option<DeviceId>,
    ) -> Result<RecoveryKit, ApiError> {
        if self.security.recovery.is_none() {
            return Err(ApiError::not_found(
                "no recovery kit exists yet; create one",
            ));
        }
        if self.security.lock.is_some() {
            let Some(passphrase) = passphrase else {
                return Err(ApiError::invalid(
                    "passphrase",
                    "the app-lock passphrase is required to rotate the keys",
                ));
            };
            self.check_current_passphrase(passphrase)?;
        }
        let now = self.clock.now_ms();
        let next = kb_keys::rotation::rotate_ark(self.rng.as_ref());
        let rck = RecoveryKey::generate(self.rng.as_ref());
        let recovery_key = rck.signing_key().verifying_key();
        let account_id = self
            .security
            .log_state
            .as_ref()
            .map(|s| s.account_id)
            .ok_or_else(|| ApiError::internal("device log without state"))?;

        // 0. Park the next key.
        self.park_next_ark(&next, passphrase)?;

        // 1a. Keyring rows that depend on the next key, under the current database key:
        // every space record re-wrapped, and a new epoch (new space key) per space.
        let generation = self.security.ark_generation + 1;
        let mut new_items = Vec::new();
        let mut spaces: Vec<SpaceId> = Vec::new();
        for item in self.security.keyring.items() {
            if let KeyringItem::Record(record) = item
                && matches!(record.subject, KeySubject::Space { .. })
            {
                if !spaces.contains(&record.space_id) {
                    spaces.push(record.space_id);
                }
                let rewrapped = kr::rewrap_space_record(
                    &record,
                    &self.ark,
                    &next,
                    &self.security.device_keys,
                    self.device.device_id,
                    now,
                    self.rng.as_ref(),
                )
                .map_err(map_keys_error)?;
                new_items.push(KeyringItem::Record(rewrapped));
            }
        }
        // A shared space's epoch is its feed's: a revocation there is a `Rekey` of the feed (below), not a
        // new local epoch; its `Space` records above are re-wrapped like every other.
        let shared_spaces = self.shared_space_map()?;
        for space_id in &spaces {
            if shared_spaces.contains_key(space_id) {
                continue;
            }
            let epoch = self.space_epoch(*space_id) + 1;
            let (_key, record) = kr::new_space_key(
                &next,
                *space_id,
                epoch,
                &self.security.device_keys,
                self.device.device_id,
                now,
                self.rng.as_ref(),
            );
            new_items.push(KeyringItem::Record(record));
        }

        // 1b. The log: the revocation, the new kit, the rotation with a box per
        // remaining device.
        let blob = rck.wrap_ark(&next, self.rng.as_ref());
        let id = rck.recovery_id();
        let recovery_blob = RecoveryBlob {
            account_id,
            generation,
            wrapped: blob.clone(),
        }
        .encode();
        let content = RotationBox {
            ark: next.clone(),
            recovery_id: id,
            recovery_blob,
        };
        let me = self.device.device_id;
        let log = self
            .security
            .log
            .as_mut()
            .ok_or_else(|| ApiError::internal("recovery kit without a device log"))?;
        let mut entries = Vec::new();
        if let Some(target) = revoke {
            entries.push(
                log.revoke_device(target, (&self.security.device_keys, me), now)
                    .map_err(|e| map_keys_error(KeysError::Log(e)))?
                    .clone(),
            );
        }
        entries.push(
            log.rotate_recovery(recovery_key, (&self.security.device_keys, me), now)
                .map_err(|e| map_keys_error(KeysError::Log(e)))?
                .clone(),
        );
        let interim = log
            .verify()
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;
        let boxes: Vec<(DeviceId, Vec<u8>)> = interim
            .devices
            .iter()
            .filter(|d| d.revoked_at.is_none() && d.device_id != me)
            .map(|d| {
                (
                    d.device_id,
                    kb_keys::rotation::seal_root_for(
                        &content,
                        generation,
                        &d.keys,
                        self.rng.as_ref(),
                    ),
                )
            })
            .collect();
        entries.push(
            log.rotate_root(generation, boxes, (&self.security.device_keys, me), now)
                .map_err(|e| map_keys_error(KeysError::Log(e)))?
                .clone(),
        );
        let state = log
            .verify()
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;

        let rows: Vec<KeyringRow> = new_items.iter().map(keyring_row).collect();
        let mut previous = self.security.previous_arks.clone();
        previous.push((self.security.ark_generation, self.ark.clone()));
        if previous.len() > PREVIOUS_ARKS_KEPT {
            let drop = previous.len() - PREVIOUS_ARKS_KEPT;
            previous.drain(..drop);
        }
        let previous_text = encode_previous_arks(&previous);
        let mut obsolete = self.security.recovery_obsolete.clone();
        if let Some(old) = self.security.recovery.as_ref().map(|r| r.id)
            && !obsolete.contains(&old)
        {
            obsolete.push(old);
        }
        let obsolete_text = encode_ids(&obsolete);
        // Devices that join after the rotation hold only the new root key: every keyring
        // item travels again under it, and every document gets a fresh snapshot under
        // its new keys on the next upload (entries sealed before stay covered).
        let states = self.db.sync_doc_state_all().map_err(map_storage_error)?;
        // The records of shared spaces travel in their feeds, sealed under space keys the rotation does
        // not touch: what was sent stays sent.
        let waiting = self.db.keyring_unpublished().map_err(map_storage_error)?;
        let sent_shared: Vec<KeyringRow> = self
            .db
            .keyring_all()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|row| shared_spaces.contains_key(&row.space_id))
            .filter(|row| {
                KeySubject::decode(&row.subject)
                    .is_ok_and(|s| !matches!(s, KeySubject::Space { .. }))
            })
            .filter(|row| !waiting.contains(row))
            .collect();
        self.db
            .transaction(|tx| {
                for row in &rows {
                    tx.keyring_put(row)?;
                }
                tx.keyring_mark_all_unpublished()?;
                tx.keyring_mark_published(&sent_shared)?;
                for state in &states {
                    tx.sync_doc_state_put(&kb_storage::SyncDocState {
                        remote_snapshot_covers: None,
                        ..state.clone()
                    })?;
                }
                for entry in &entries {
                    tx.device_log_append(&DeviceLogRow {
                        seq: entry.seq,
                        entry: entry.encode(),
                        hash: entry.hash(),
                    })?;
                }
                tx.meta_set(META_ARK_GENERATION, &generation.to_string())?;
                tx.meta_set(META_PREVIOUS_ARKS, &previous_text)?;
                tx.meta_set(META_RECOVERY_OBSOLETE, &obsolete_text)?;
                tx.meta_set(META_RECOVERY_PUBLISHED, "0")?;
                tx.meta_set(META_ROTATION_PENDING, "1")
            })
            .map_err(map_storage_error)?;
        for item in new_items {
            self.security.keyring.merge(item);
        }
        self.security.previous_arks = previous;
        self.security.recovery_obsolete = obsolete;
        self.security.ark_generation = generation;
        self.security.log_state = Some(state);

        // 2 + 3. Rekey the database and promote the keystore.
        self.switch_ark(next, passphrase)?;

        // 4. The new recovery blob.
        let created_at = self
            .security
            .recovery
            .as_ref()
            .map_or(now, |r| r.created_at);
        self.db
            .transaction(|tx| {
                tx.meta_set(META_RECOVERY_ID, &hex(&id))?;
                tx.meta_set(META_RECOVERY_BLOB, &hex(&blob.to_bytes()))?;
                tx.meta_set(META_RECOVERY_VERIFIED, "")?;
                tx.meta_set(META_RECOVERY_REISSUED, &now.to_string())?;
                tx.meta_set(META_RECOVERY_STALE, "0")?;
                tx.meta_set(META_ROTATION_PENDING, "0")
            })
            .map_err(map_storage_error)?;
        self.security.recovery = Some(RecoveryMeta {
            id,
            blob,
            created_at,
            verified_at: None,
            reissued_at: Some(now),
        });
        self.security.recovery_stale = false;
        self.security.recovery_published = false;
        self.security.space_keys.clear();
        self.events.push(Event::SecurityChanged);
        if let Some(target) = revoke {
            self.events.push(Event::DevicesChanged {
                added: Vec::new(),
                revoked: vec![target],
                this_device_revoked: false,
                rotated: true,
            });
        }
        self.sync_note_local_change();
        tracing::info!(
            generation,
            revoked = revoke.is_some(),
            "root key rotated and recovery kit re-issued"
        );
        if revoke.is_some() {
            // ADR-0022 branches per kind of space: one `Rekey` per shared feed (ADR-0044).
            self.rekey_shared_spaces_after_revocation();
        }
        Ok(RecoveryKit {
            words: rck.words(),
            code: rck.code(),
            account_id,
            created_at: now,
        })
    }

    pub(crate) fn park_next_ark(
        &self,
        next: &Ark,
        passphrase: Option<&str>,
    ) -> Result<(), ApiError> {
        match (self.security.lock, passphrase) {
            (Some(params), Some(passphrase)) => {
                let blob = kb_crypto::wrap_with_passphrase(
                    next.secret(),
                    passphrase.as_bytes(),
                    params,
                    &next_locked_subject(&self.install_id),
                    self.rng.as_ref(),
                )
                .map_err(|e| map_keys_error(KeysError::Crypto(e)))?;
                self.keystore
                    .set(&next_locked_entry_name(&self.install_id), &blob.to_bytes())
                    .map_err(|e| map_keys_error(KeysError::Keystore(e)))
            }
            _ => self
                .keystore
                .set(&next_entry_name(&self.install_id), next.secret().expose())
                .map_err(|e| map_keys_error(KeysError::Keystore(e))),
        }
    }

    /// Verifies the app-lock passphrase against the keystore blob (Argon2id cost).
    pub(crate) fn check_current_passphrase(&self, passphrase: &str) -> Result<(), ApiError> {
        let state =
            lk::load_ark_state(self.keystore.as_ref(), &self.install_id).map_err(map_keys_error)?;
        let ArkState::Locked(locked) = state else {
            return Err(ApiError::internal(
                "the app lock is enabled but the keystore holds no locked key",
            ));
        };
        let ark = locked
            .unlock_with_minimum(passphrase, &self.lock_params)
            .map_err(map_keys_error)?;
        if ark != self.ark {
            return Err(ApiError::crypto(
                "the keystore's locked key is not this workspace's root key",
            ));
        }
        Ok(())
    }

    fn app_lock_enable(&mut self, passphrase: &str) -> Result<(), ApiError> {
        lk::check_passphrase(passphrase).map_err(map_keys_error)?;
        if self.security.lock.is_some() {
            return Err(ApiError::already_exists("the app lock is already enabled"));
        }
        let params = self.lock_params;
        lk::enable_lock(
            self.keystore.as_ref(),
            &self.install_id,
            &self.ark,
            passphrase,
            params,
            self.rng.as_ref(),
        )
        .map_err(map_keys_error)?;
        self.security.lock = Some(params);
        self.events.push(Event::SecurityChanged);
        tracing::info!("app lock enabled");
        Ok(())
    }

    fn app_lock_disable(&mut self, passphrase: &str) -> Result<(), ApiError> {
        if self.security.lock.is_none() {
            return Err(ApiError::invalid("lock", "the app lock is not enabled"));
        }
        self.check_current_passphrase(passphrase)?;
        lk::disable_lock(
            self.keystore.as_ref(),
            &self.install_id,
            &self.ark,
            self.rng.as_ref(),
        )
        .map_err(map_keys_error)?;
        self.security.lock = None;
        self.events.push(Event::SecurityChanged);
        tracing::info!("app lock disabled");
        Ok(())
    }

    fn app_lock_change(&mut self, current: &str, new: &str) -> Result<(), ApiError> {
        let Some(params) = self.security.lock else {
            return Err(ApiError::invalid("lock", "the app lock is not enabled"));
        };
        lk::check_passphrase(new).map_err(|_| {
            ApiError::invalid(
                "new",
                format!(
                    "must be {} to {} bytes",
                    lk::PASSPHRASE_MIN_BYTES,
                    lk::PASSPHRASE_MAX_BYTES
                ),
            )
        })?;
        lk::change_passphrase(
            self.keystore.as_ref(),
            &self.install_id,
            &self.ark,
            current,
            new,
            params,
            self.rng.as_ref(),
        )
        .map_err(|err| match err {
            KeysError::WrongPassphrase => ApiError::invalid("current", "wrong passphrase"),
            other => map_keys_error(other),
        })?;
        self.events.push(Event::SecurityChanged);
        Ok(())
    }

    pub(crate) fn device_list(&self) -> Result<Vec<DeviceEntry>, ApiError> {
        let Some(state) = &self.security.log_state else {
            return Ok(Vec::new());
        };
        let names = self.account_device_names();
        Ok(state
            .devices
            .iter()
            .map(|d| {
                let is_this_device = d.device_id == self.device.device_id;
                let name = if is_this_device {
                    self.device.name.clone()
                } else {
                    names
                        .get(&d.device_id)
                        .cloned()
                        .or_else(|| {
                            lg::decrypt_name(&self.ark, d.device_id, &d.encrypted_name).ok()
                        })
                        .unwrap_or_else(|| "Device".to_owned())
                };
                DeviceEntry {
                    device_id: d.device_id,
                    name,
                    added_at: d.added_at,
                    added_via_kit: d.added_via_kit,
                    revoked_at: d.revoked_at,
                    is_this_device,
                    signing_key: d.keys.signing.to_hex(),
                    added_by: match d.added_by {
                        kb_keys::Signer::Device(by) if by != d.device_id => Some(by),
                        _ => None,
                    },
                    revoked_by: d.revoked_by,
                }
            })
            .collect())
    }

    /// Device names kept in the account document (they survive root key rotation).
    fn account_device_names(&self) -> HashMap<DeviceId, String> {
        self.docs
            .account
            .as_ref()
            .map(|a| a.model.device_names().into_iter().collect())
            .unwrap_or_default()
    }

    /// The error every request gets while the core is locked (used by shells).
    #[must_use]
    pub fn locked_error() -> ApiError {
        ApiError::new(ErrorCode::Locked, "the workspace is locked")
    }
}

/// A keyring row from an item.
pub(crate) fn keyring_row(item: &KeyringItem) -> KeyringRow {
    KeyringRow {
        space_id: item.space_id(),
        subject: item.subject().encode(),
        record: item.encode(),
        tombstone: matches!(item, KeyringItem::Tombstone(_)),
    }
}

/// `gen:hex,gen:hex` of previous root keys.
fn parse_previous_arks(text: &str) -> Vec<(u32, Ark)> {
    text.split(',')
        .filter_map(|pair| {
            let (generation, hex) = pair.split_once(':')?;
            let bytes: [u8; 32] = decode_hex(hex)?.try_into().ok()?;
            Some((
                generation.parse().ok()?,
                Ark::from_secret(Secret32::from_bytes(bytes)),
            ))
        })
        .collect()
}

pub(crate) fn encode_previous_arks(arks: &[(u32, Ark)]) -> String {
    arks.iter()
        .map(|(generation, ark)| format!("{generation}:{}", hex(ark.secret().expose())))
        .collect::<Vec<_>>()
        .join(",")
}

/// Comma-separated 32-byte hex ids.
pub(crate) fn parse_ids(text: &str) -> Vec<[u8; 32]> {
    text.split(',')
        .filter_map(|id| decode_hex(id)?.try_into().ok())
        .collect()
}

pub(crate) fn encode_ids(ids: &[[u8; 32]]) -> String {
    ids.iter().map(|id| hex(id)).collect::<Vec<_>>().join(",")
}

fn meta_i64(db: &Db, key: &str) -> Result<Option<i64>, ApiError> {
    let value = db.meta_get(key).map_err(map_storage_error)?;
    Ok(value
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse::<i64>().ok()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}
