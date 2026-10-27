//! Recovery with the kit from a server (Milestone 7, ADR-0022): the sans-IO state
//! machine of a fresh installation that fetches the recovery blob and the device log,
//! signs its own `DeviceAdded` with the recovery key and installs the account. The
//! installation itself ([`Core::install_joined_account`]) is shared with pairing.

use kb_crypto::WrappedKey;
use kb_keys::recovery::RecoveryBlob;
use kb_keys::{Ark, DeviceLog, KeysError, LogEntry, LogSigner, RecoveryKey};
use kb_storage::{DeviceLogRow, SecurityQueries, SyncQueries, WorkspaceRow};
use kb_types::{
    AccountId, ApiError, Event, RecoveryInfo, RecoveryState, Request, Response, WorkspaceId,
};

use crate::security::{
    META_ACCOUNT_ID, META_ARK_GENERATION, META_RECOVERY_BLOB, META_RECOVERY_CREATED,
    META_RECOVERY_ID, META_RECOVERY_PUBLISHED, META_RECOVERY_STALE, hex,
};
use crate::{Core, map_keys_error, map_storage_error};

/// The workspace name of an account recovered through the kit (the name is not in the
/// log; the user renames it).
const RECOVERED_WORKSPACE_NAME: &str = "My notes";

/// The recovery this core runs, if any.
pub(crate) struct RecoverySession {
    pub(crate) state: RecoveryState,
    pub(crate) server_url: String,
    pub(crate) error: Option<String>,
    rck: RecoveryKey,
    ark: Option<Ark>,
    blob: Option<RecoveryBlob>,
    /// This device's `DeviceAdded`, once the log was taken: its sequence, to mark it
    /// published.
    entry_seq: Option<u64>,
}

/// Everything a device needs to join an account it did not create: the pairing bundle
/// or the recovered blob plus log, in one shape.
pub(crate) struct JoinedAccount {
    pub(crate) ark: Ark,
    pub(crate) ark_generation: u32,
    pub(crate) account_id: AccountId,
    /// The whole log, this device's entry included.
    pub(crate) log: Vec<LogEntry>,
    /// Entries the server already holds (`seq` ≤ this are marked published).
    pub(crate) published_up_to: Option<u64>,
    pub(crate) recovery_id: [u8; 32],
    pub(crate) recovery_blob: WrappedKey,
    pub(crate) recovery_created_at: i64,
    pub(crate) workspace: WorkspaceRow,
    pub(crate) server_url: String,
    /// The kit was typed on this device (recovery): suggest a re-issue.
    pub(crate) kit_stale: bool,
}

impl Core {
    /// Routes the recovery requests.
    pub(crate) fn dispatch_recovery(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::RecoveryStart { server_url, kit } => {
                self.recovery_start(&server_url, &kit)?;
                Ok(Response::Recovery(self.recovery_info()))
            }
            Request::RecoveryStatus => Ok(Response::Recovery(self.recovery_info())),
            Request::RecoveryCancel => {
                if self
                    .recovery
                    .as_ref()
                    .is_some_and(|s| s.state != RecoveryState::Done)
                {
                    self.recovery = None;
                    self.events.push(Event::RecoveryChanged);
                }
                Ok(Response::Recovery(self.recovery_info()))
            }
            other => Err(ApiError::internal(format!(
                "{} is not a recovery request",
                other.kind()
            ))),
        }
    }

    /// The recovery as the UI shows it.
    pub fn recovery_info(&self) -> RecoveryInfo {
        match &self.recovery {
            Some(session) => RecoveryInfo {
                state: session.state,
                server_url: Some(session.server_url.clone()),
                error: session.error.clone(),
            },
            None => RecoveryInfo {
                state: RecoveryState::Idle,
                server_url: None,
                error: None,
            },
        }
    }

    fn recovery_start(&mut self, server_url: &str, kit: &str) -> Result<(), ApiError> {
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some()
        {
            return Err(ApiError::already_exists(
                "this installation already has a workspace; recovery needs a fresh one",
            ));
        }
        let server_url = validate_server_url(server_url)?;
        let rck = RecoveryKey::from_kit(kit.trim()).map_err(map_keys_error)?;
        self.recovery = Some(RecoverySession {
            state: RecoveryState::Fetching,
            server_url,
            error: None,
            rck,
            ark: None,
            blob: None,
            entry_seq: None,
        });
        self.events.push(Event::RecoveryChanged);
        self.events.push(Event::RecoveryRequested);
        Ok(())
    }

    // ----- for the shell's recovery task (kb-net) ---------------------------------------

    /// The lookup id of the recovery blob.
    pub fn recovery_id(&self) -> Result<Option<[u8; 32]>, ApiError> {
        Ok(self.recovery.as_ref().map(|s| s.rck.recovery_id()))
    }

    /// The account the recovery blob named (known after `recovery_blob_received`).
    pub fn recovery_account_id(&self) -> Result<Option<AccountId>, ApiError> {
        Ok(self
            .recovery
            .as_ref()
            .and_then(|s| s.blob.as_ref())
            .map(|b| b.account_id))
    }

    /// The blob fetched from the server: the root key comes out of it.
    pub fn recovery_blob_received(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        let session = self
            .recovery
            .as_mut()
            .ok_or_else(|| ApiError::not_found("no recovery is running"))?;
        let blob = match RecoveryBlob::decode(bytes) {
            Ok(blob) => blob,
            Err(err) => {
                session.state = RecoveryState::Failed;
                session.error = Some("the server's recovery data is not readable".to_owned());
                self.events.push(Event::RecoveryChanged);
                return Err(map_keys_error(err));
            }
        };
        let ark = match session.rck.unwrap_ark(&blob.wrapped) {
            Ok(ark) => ark,
            Err(err) => {
                session.state = RecoveryState::Failed;
                session.error = Some("the kit does not open this account's key".to_owned());
                self.events.push(Event::RecoveryChanged);
                return Err(map_keys_error(err));
            }
        };
        session.ark = Some(ark);
        session.blob = Some(blob);
        session.state = RecoveryState::Joining;
        self.events.push(Event::RecoveryChanged);
        Ok(())
    }

    /// The account's device log fetched from the server: this device adds itself with
    /// the recovery key, installs the account and answers the entry to publish.
    pub fn recovery_log_received(&mut self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError> {
        let now = self.clock.now_ms();
        let Some(session) = self.recovery.as_ref() else {
            return Err(ApiError::not_found("no recovery is running"));
        };
        let (Some(ark), Some(blob)) = (session.ark.clone(), session.blob.clone()) else {
            return Err(ApiError::internal("the recovery blob was not received yet"));
        };
        let server_url = session.server_url.clone();
        let signing = session.rck.signing_key();
        let recovery_id = session.rck.recovery_id();
        let result = (|| -> Result<(Vec<u8>, u64), ApiError> {
            let decoded = entries
                .iter()
                .map(|bytes| LogEntry::decode(bytes))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| map_keys_error(KeysError::Log(e)))?;
            let mut log =
                DeviceLog::from_entries(decoded).map_err(|e| map_keys_error(KeysError::Log(e)))?;
            let state = log
                .verify()
                .map_err(|e| map_keys_error(KeysError::Log(e)))?;
            if state.account_id != blob.account_id {
                return Err(ApiError::crypto(
                    "the device log does not belong to the recovered account",
                ));
            }
            let published_up_to = log.entries().last().map(|e| e.seq);
            let name = kb_keys::log::encrypt_name(
                &ark,
                self.device.device_id,
                &self.device.name,
                self.rng.as_ref(),
            );
            let entry = log
                .add_device(
                    self.device.device_id,
                    self.security.device_keys.public(),
                    name,
                    LogSigner::Recovery(&signing),
                    now,
                )
                .map_err(|e| map_keys_error(KeysError::Log(e)))?
                .clone();
            let workspace = WorkspaceRow {
                id: WorkspaceId::generate(self.rng.as_ref()),
                name: RECOVERED_WORKSPACE_NAME.to_owned(),
                created_at: now,
            };
            self.install_joined_account(JoinedAccount {
                ark,
                ark_generation: blob.generation,
                account_id: blob.account_id,
                log: log.entries().to_vec(),
                published_up_to,
                recovery_id,
                recovery_blob: blob.wrapped.clone(),
                recovery_created_at: now,
                workspace,
                server_url,
                kit_stale: true,
            })?;
            Ok((entry.encode(), entry.seq))
        })();
        match result {
            Ok((bytes, seq)) => {
                if let Some(session) = self.recovery.as_mut() {
                    session.entry_seq = Some(seq);
                }
                self.events.push(Event::RecoveryChanged);
                Ok(bytes)
            }
            Err(err) => {
                if let Some(session) = self.recovery.as_mut() {
                    session.state = RecoveryState::Failed;
                    session.error = Some(err.message.clone());
                }
                self.events.push(Event::RecoveryChanged);
                Err(err)
            }
        }
    }

    /// The server took the new log entry: the recovery is complete.
    pub fn recovery_entry_published(&mut self) -> Result<(), ApiError> {
        let seq = self
            .recovery
            .as_ref()
            .and_then(|s| s.entry_seq)
            .ok_or_else(|| ApiError::not_found("no recovery entry waits for publication"))?;
        self.sync_log_published(seq)?;
        if let Some(session) = self.recovery.as_mut() {
            session.state = RecoveryState::Done;
        }
        self.events.push(Event::RecoveryChanged);
        self.events.push(Event::SyncRequested);
        Ok(())
    }

    /// The task gave up.
    pub fn recovery_failed(&mut self, message: &str) -> Result<(), ApiError> {
        if let Some(session) = self.recovery.as_mut()
            && session.state != RecoveryState::Done
        {
            session.state = RecoveryState::Failed;
            session.error = Some(message.to_owned());
            self.events.push(Event::RecoveryChanged);
        }
        Ok(())
    }

    // ----- joining an account ------------------------------------------------------------

    /// Installs an account this device did not create (pairing bundle or recovery):
    /// the workspace row, the device log, the recovery metadata, the root key (the
    /// empty database is re-keyed), then the sync configuration. The log must list
    /// this device as active.
    pub(crate) fn install_joined_account(&mut self, joined: JoinedAccount) -> Result<(), ApiError> {
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some()
        {
            return Err(ApiError::already_exists(
                "this installation already has a workspace",
            ));
        }
        let log = DeviceLog::from_entries(joined.log.clone())
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;
        let state = log
            .verify()
            .map_err(|e| map_keys_error(KeysError::Log(e)))?;
        if state.account_id != joined.account_id {
            return Err(ApiError::crypto(
                "the device log does not belong to the account",
            ));
        }
        if state.active(self.device.device_id).is_none() {
            return Err(ApiError::crypto("the device log does not list this device"));
        }
        let account_text = joined.account_id.to_string();
        let recovery_hex = hex(&joined.recovery_id);
        let blob_hex = hex(&joined.recovery_blob.to_bytes());
        let created_text = joined.recovery_created_at.to_string();
        let generation_text = joined.ark_generation.to_string();
        let stale = if joined.kit_stale { "1" } else { "0" };
        self.db
            .transaction(|tx| {
                if !tx.workspace_insert(&joined.workspace)? {
                    return Err(kb_storage::StorageError::InvalidData(
                        "workspace exists".into(),
                    ));
                }
                for entry in &joined.log {
                    tx.device_log_append(&DeviceLogRow {
                        seq: entry.seq,
                        entry: entry.encode(),
                        hash: entry.hash(),
                    })?;
                }
                if let Some(up_to) = joined.published_up_to {
                    tx.device_log_mark_published(up_to)?;
                }
                tx.meta_set(META_ACCOUNT_ID, &account_text)?;
                tx.meta_set(META_RECOVERY_ID, &recovery_hex)?;
                tx.meta_set(META_RECOVERY_BLOB, &blob_hex)?;
                tx.meta_set(META_RECOVERY_CREATED, &created_text)?;
                tx.meta_set(META_RECOVERY_STALE, stale)?;
                tx.meta_set(META_RECOVERY_PUBLISHED, "0")?;
                tx.meta_set(META_ARK_GENERATION, &generation_text)?;
                Ok(())
            })
            .map_err(map_storage_error)?;
        self.park_and_switch(joined.ark)?;
        self.security.lock = None;
        self.docs = crate::documents::load_documents(&self.db, self.rng.as_ref())?;
        crate::documents::verify_index(&mut self.db, &self.docs)?;
        self.security = Core::load_security(
            &self.db,
            &self.ark,
            self.security.device_keys.clone(),
            self.device.device_id,
            None,
        )?;
        self.ensure_account()?;
        self.verify_text_index()?;
        self.events.push(Event::WorkspaceCreated {
            id: joined.workspace.id,
        });
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::SecurityChanged);
        self.sync_configure(Some(&joined.server_url), None)?;
        tracing::info!("account joined; sync starts");
        Ok(())
    }
}

/// `http(s)://host[:port][/path]`, trimmed, no user info, query, fragment or spaces.
pub(crate) fn validate_server_url(url: &str) -> Result<String, ApiError> {
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
    Ok(url.to_owned())
}
