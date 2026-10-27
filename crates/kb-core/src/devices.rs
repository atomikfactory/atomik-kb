//! Devices (Milestone 7, ADR-0022): revocation with the root key and space epoch
//! rotation, rotations received from other devices, the recovery blob for the server,
//! and what a device does when the log says it was revoked.

use kb_keys::rotation::RotationBox;
use kb_keys::{KeysError, LogEntry, LogEntryBody, LogState};
use kb_storage::SecurityQueries;
use kb_types::{ApiError, DeviceId, Event, RecoveryKit, Request, Response};

use crate::security::{
    META_ARK_GENERATION, META_PENDING_ROTATION, META_PREVIOUS_ARKS, META_RECOVERY_BLOB,
    META_RECOVERY_ID, META_RECOVERY_PUBLISHED, META_RECOVERY_REISSUED, META_RECOVERY_STALE,
    META_REVOKED_AT, PREVIOUS_ARKS_KEPT, RecoveryMeta, encode_ids, encode_previous_arks, hex,
};
use crate::{Core, map_keys_error, map_storage_error};

/// A recovery blob to upload: its lookup id and its bytes.
pub type RecoveryUpload = ([u8; 32], Vec<u8>);

impl Core {
    /// Routes the device requests.
    pub(crate) fn dispatch_devices(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::DeviceRevoke {
                device_id,
                passphrase,
            } => Ok(Response::RecoveryKit(
                self.device_revoke(device_id, passphrase.as_deref())?,
            )),
            Request::RotationApply { passphrase } => {
                self.rotation_apply(passphrase.as_deref())?;
                Ok(Response::Security(self.security_info()))
            }
            other => Err(ApiError::internal(format!(
                "{} is not a device request",
                other.kind()
            ))),
        }
    }

    /// Revokes another active device: `DeviceRevoked`, then the rotation with the new
    /// kit (`rotate_root`).
    fn device_revoke(
        &mut self,
        device_id: DeviceId,
        passphrase: Option<&str>,
    ) -> Result<RecoveryKit, ApiError> {
        if device_id == self.device.device_id {
            return Err(ApiError::invalid(
                "deviceId",
                "a device cannot remove itself; use another device",
            ));
        }
        let active = self
            .security
            .log_state
            .as_ref()
            .is_some_and(|s| s.active(device_id).is_some());
        if !active {
            return Err(ApiError::invalid(
                "deviceId",
                "this device is not an active device of the account",
            ));
        }
        self.rotate_root(passphrase, Some(device_id))
    }

    /// Finishes a rotation another device made while the app lock here needed the
    /// passphrase to store the new root key.
    fn rotation_apply(&mut self, passphrase: Option<&str>) -> Result<(), ApiError> {
        let Some((generation, sealed)) = self.security.pending_rotation.clone() else {
            return Err(ApiError::not_found("no rotation is waiting"));
        };
        if self.security.lock.is_some() {
            let Some(passphrase) = passphrase else {
                return Err(ApiError::invalid(
                    "passphrase",
                    "the app-lock passphrase is required to finish the rotation",
                ));
            };
            self.check_current_passphrase(passphrase)?;
        }
        self.apply_rotation(generation, &sealed, passphrase)
    }

    // ----- what the log tells this device -------------------------------------------------

    /// After the device log grew through sync: devices that joined or were revoked, this
    /// device's own revocation, and root key rotations with a box for this device.
    pub(crate) fn after_log_change(
        &mut self,
        before: Option<&LogState>,
        after: &LogState,
        new_entries: &[LogEntry],
    ) -> Result<(), ApiError> {
        let me = self.device.device_id;
        let known_before =
            |id: DeviceId| before.is_some_and(|s| s.devices.iter().any(|d| d.device_id == id));
        let revoked_before = |id: DeviceId| before.is_some_and(|s| s.revoked_at(id).is_some());
        let added: Vec<DeviceId> = after
            .devices
            .iter()
            .filter(|d| !known_before(d.device_id))
            .map(|d| d.device_id)
            .collect();
        let revoked: Vec<DeviceId> = after
            .devices
            .iter()
            .filter(|d| d.revoked_at.is_some() && !revoked_before(d.device_id))
            .map(|d| d.device_id)
            .collect();
        let mut rotated = false;
        for entry in new_entries {
            if let LogEntryBody::RootRotated {
                generation, boxes, ..
            } = &entry.body
            {
                rotated = true;
                if let Some((_, sealed)) = boxes.iter().find(|(id, _)| *id == me) {
                    self.receive_rotation(*generation, sealed)?;
                }
            }
        }
        let this_device_revoked = revoked.contains(&me);
        if this_device_revoked {
            self.mark_revoked()?;
        }
        if !added.is_empty() || !revoked.is_empty() || rotated {
            let entries = self.device_list()?;
            self.events.push(Event::DevicesChanged {
                added: entries
                    .into_iter()
                    .filter(|e| added.contains(&e.device_id))
                    .collect(),
                revoked,
                this_device_revoked,
                rotated,
            });
        }
        Ok(())
    }

    /// A `RootRotated` box for this device: applied now, or parked until the app-lock
    /// passphrase is given (`RotationApply`).
    fn receive_rotation(&mut self, generation: u32, sealed: &[u8]) -> Result<(), ApiError> {
        if generation <= self.security.ark_generation {
            tracing::debug!(generation, "rotation already applied");
            return Ok(());
        }
        if self.security.lock.is_some() {
            self.db
                .meta_set(
                    META_PENDING_ROTATION,
                    &format!("{generation}:{}", hex(sealed)),
                )
                .map_err(map_storage_error)?;
            self.security.pending_rotation = Some((generation, sealed.to_vec()));
            self.events.push(Event::SecurityChanged);
            tracing::info!(generation, "a root key rotation waits for the passphrase");
            return Ok(());
        }
        self.apply_rotation(generation, sealed, None)
    }

    /// Opens the box, keeps the current root key as a reading key, re-keys the database
    /// and takes the new recovery blob the rotating device issued.
    fn apply_rotation(
        &mut self,
        generation: u32,
        sealed: &[u8],
        passphrase: Option<&str>,
    ) -> Result<(), ApiError> {
        let content: RotationBox =
            kb_keys::rotation::open_root_box(&self.security.device_keys, generation, sealed)
                .map_err(map_keys_error)?;
        let blob = kb_keys::recovery::RecoveryBlob::decode(&content.recovery_blob)
            .map_err(map_keys_error)?;
        let now = self.clock.now_ms();
        self.park_next_ark(&content.ark, passphrase)?;
        let mut previous = self.security.previous_arks.clone();
        previous.push((self.security.ark_generation, self.ark.clone()));
        if previous.len() > PREVIOUS_ARKS_KEPT {
            let drop = previous.len() - PREVIOUS_ARKS_KEPT;
            previous.drain(..drop);
        }
        let previous_text = encode_previous_arks(&previous);
        let created_at = self
            .security
            .recovery
            .as_ref()
            .map_or(now, |r| r.created_at);
        self.db
            .transaction(|tx| {
                tx.meta_set(META_ARK_GENERATION, &generation.to_string())?;
                tx.meta_set(META_PREVIOUS_ARKS, &previous_text)?;
                tx.meta_set(META_RECOVERY_ID, &hex(&content.recovery_id))?;
                tx.meta_set(META_RECOVERY_BLOB, &hex(&blob.wrapped.to_bytes()))?;
                tx.meta_set(META_RECOVERY_REISSUED, &now.to_string())?;
                tx.meta_set(META_RECOVERY_STALE, "0")?;
                tx.meta_set(META_RECOVERY_PUBLISHED, "0")?;
                tx.meta_set(META_PENDING_ROTATION, "")
            })
            .map_err(map_storage_error)?;
        self.security.previous_arks = previous;
        self.security.ark_generation = generation;
        self.switch_ark(content.ark, passphrase)?;
        self.security.recovery = Some(RecoveryMeta {
            id: content.recovery_id,
            blob: blob.wrapped,
            created_at,
            verified_at: None,
            reissued_at: Some(now),
        });
        self.security.recovery_stale = false;
        self.security.recovery_published = false;
        self.security.pending_rotation = None;
        self.security.space_keys.clear();
        self.events.push(Event::SecurityChanged);
        tracing::info!(generation, "root key rotation from another device applied");
        Ok(())
    }

    /// The log says this device was revoked: sync stops, the data stays.
    fn mark_revoked(&mut self) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        self.db
            .meta_set(META_REVOKED_AT, &now.to_string())
            .map_err(map_storage_error)?;
        self.security.revoked = true;
        self.sync_configure(None, None)?;
        self.events.push(Event::SecurityChanged);
        tracing::warn!("this device was removed from the account");
        Ok(())
    }

    // ----- the recovery blob on the server --------------------------------------------------

    /// The recovery blob to upload (`(recovery_id, bytes)`) when the configured server
    /// does not hold the current one yet.
    pub fn sync_recovery_blob(&self) -> Result<Option<RecoveryUpload>, ApiError> {
        if self.security.recovery_published || self.sync_server_url()?.is_none() {
            return Ok(None);
        }
        let (Some(meta), Some(state)) = (&self.security.recovery, &self.security.log_state) else {
            return Ok(None);
        };
        let blob = kb_keys::recovery::RecoveryBlob {
            account_id: state.account_id,
            generation: self.security.ark_generation,
            wrapped: meta.blob.clone(),
        };
        Ok(Some((meta.id, blob.encode())))
    }

    /// The server took the recovery blob.
    pub fn sync_recovery_published(&mut self) -> Result<(), ApiError> {
        self.db
            .meta_set(META_RECOVERY_PUBLISHED, "1")
            .map_err(map_storage_error)?;
        self.security.recovery_published = true;
        self.events.push(Event::SecurityChanged);
        Ok(())
    }

    /// Recovery ids of earlier kits the server should delete.
    pub fn sync_recovery_obsolete(&self) -> Result<Vec<[u8; 32]>, ApiError> {
        Ok(self.security.recovery_obsolete.clone())
    }

    /// The server deleted an obsolete recovery blob.
    pub fn sync_recovery_deleted(&mut self, id: [u8; 32]) -> Result<(), ApiError> {
        self.security.recovery_obsolete.retain(|known| *known != id);
        let text = encode_ids(&self.security.recovery_obsolete);
        self.db
            .meta_set(crate::security::META_RECOVERY_OBSOLETE, &text)
            .map_err(map_storage_error)
    }
}

/// Keeps the compiler honest about the entry types this module interprets.
#[allow(dead_code)]
fn _entry_kinds(entry: &LogEntry) -> Result<(), KeysError> {
    let _ = &entry.body;
    Ok(())
}
