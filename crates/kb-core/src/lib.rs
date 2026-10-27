//! The core of atomik-kb: composition root and the single API surface (ADR-0015).
//!
//! A shell constructs [`CoreDeps`] from platform pieces, calls [`Core::open`], then
//! drives everything through [`Core::dispatch`] and [`Core::drain_events`]. The core
//! is synchronous and performs no I/O beyond the database handed to it; shells own
//! threads, network and files.
//!
//! Milestone 1: open sequence (ARK, DB key, encrypted DB, migrations, device
//! identity) and the workspace/settings/device handlers. Milestone 2: the account
//! and structure documents (spaces, page tree) in [`documents`]. Milestone 3: page
//! documents, text and link indexes, search in [`pages`]. Typed page properties (ADR-0026):
//! the schema and the values in the structure document, the derived rows and the queries in
//! `properties`.

#![forbid(unsafe_code)]

mod ai;
mod attachments;
mod bulk;
pub use attachments::{AttachmentSink, MAX_SEALED_CHUNK};
mod backup;
pub mod backup_schedule;
mod comments;
mod devices;
mod documents;
mod duplicate;
mod export;
mod feeds;
mod forkout;
mod history;
mod icons;
mod import;
mod invites;
mod links;
mod mcp;
mod membership;
mod mentions;
mod notifications;
mod pages;
mod pairing;
mod presence;
mod profile;
mod properties;
mod recovery;
mod reminders;
pub use reminders::{CATCH_UP_SECONDS, FIRES_PER_TICK, LAST_CHECKED_SETTING, MAX_REMINDERS_DUE};
mod security;
mod sharing;
mod space_move;
mod sync;
mod template_create;
mod templates;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod test_support_interop;
#[cfg(test)]
mod tests_ai;
#[cfg(test)]
mod tests_appearance;
#[cfg(test)]
mod tests_attachments;
#[cfg(test)]
mod tests_authorship;
#[cfg(test)]
mod tests_bulk;
#[cfg(test)]
mod tests_cover;
#[cfg(test)]
mod tests_duplicate;
#[cfg(test)]
mod tests_export;
#[cfg(test)]
mod tests_favorites;
#[cfg(test)]
mod tests_formulas;
#[cfg(test)]
mod tests_history;
#[cfg(test)]
mod tests_icons;
#[cfg(test)]
mod tests_import;
#[cfg(test)]
mod tests_import_spaces;
#[cfg(test)]
mod tests_interop;
#[cfg(test)]
mod tests_mcp;
#[cfg(test)]
mod tests_pages;
#[cfg(test)]
mod tests_properties;
#[cfg(test)]
mod tests_relations;
#[cfg(test)]
mod tests_reminders;
#[cfg(test)]
mod tests_search;
#[cfg(test)]
mod tests_security;
#[cfg(test)]
mod tests_sharing;
#[cfg(test)]
mod tests_space_move;
#[cfg(test)]
mod tests_structure;
#[cfg(test)]
mod tests_sync;
#[cfg(test)]
mod tests_template_create;
#[cfg(test)]
mod tests_templates;
#[cfg(test)]
mod tests_trash;
mod transfers;
mod trash;
pub use trash::{DEFAULT_TRASH_RETENTION_DAYS, TRASH_RETENTION_SETTING};
mod validate;

use kb_keys::{Ark, KeysError, Keystore, KeystoreError};
use kb_storage::{Db, DbOpener, DeviceRow, StorageError, WorkspaceRow};
use kb_types::{
    API_VERSION, ApiError, AppStatus, CORE_VERSION, Clock, DeviceInfo, Event, InstallId, Request,
    Response, Rng, Workspace, WorkspaceId,
};

pub use backup::{BACKUP_FORMAT, BACKUP_MAGIC};
pub use documents::CompactionPolicy;
pub use pages::MAX_DOC_UPDATE_BYTES;
pub use security::{LockedCore, Opened};
pub use sync::ACCOUNT_UNKNOWN_MESSAGE;

/// Maximum serialised size of a setting value.
pub const MAX_SETTING_BYTES: usize = 64 * 1024;

/// Everything the core needs from the platform.
pub struct CoreDeps {
    /// Local installation id (plaintext, non-secret; names the keystore entry).
    pub install_id: InstallId,
    /// Platform name for display and diagnostics, e.g. `"windows"`, `"web"`.
    pub platform: String,
    /// Where data lives, for display only.
    pub data_dir: String,
    /// Platform secret storage.
    pub keystore: Box<dyn Keystore>,
    /// How to open the database.
    pub opener: Box<dyn DbOpener>,
    /// Time source.
    pub clock: Box<dyn Clock>,
    /// Randomness source.
    pub rng: Box<dyn Rng>,
    /// Argon2id parameters of the app lock. Shells pass
    /// `kb_crypto::Argon2Params::DEFAULT`; they are also the minimum this core accepts
    /// when unlocking (weaker stored blobs are refused). Tests use small values.
    pub lock_params: kb_crypto::Argon2Params,
    /// Where encrypted attachment chunks live (ADR-0020): the native file store under
    /// the data directory, an in-memory store in tests.
    pub blob_store: Box<dyn kb_storage::BlobStore>,
}

/// One open workspace. Owned by exactly one shell-side single-writer lock.
pub struct Core {
    db: Db,
    clock: Box<dyn Clock>,
    rng: Box<dyn Rng>,
    keystore: Box<dyn Keystore>,
    ark: Ark,
    install_id: InstallId,
    opener: Box<dyn DbOpener>,
    platform: String,
    data_dir: String,
    device: DeviceRow,
    docs: documents::Documents,
    policy: CompactionPolicy,
    events: Vec<Event>,
    security: security::Security,
    lock_params: kb_crypto::Argon2Params,
    blob_store: Box<dyn kb_storage::BlobStore>,
    /// Attachment uploads between `AttachmentBegin` and `AttachmentFinish`.
    uploads: std::collections::HashMap<kb_types::AttachmentId, attachments::Upload>,
    /// Sync runtime state (Milestone 6); the durable part is in `sync_config`.
    sync: sync::SyncRuntime,
    /// The pairing session (Milestone 7), while one runs.
    pairing: Option<pairing::PairingSession>,
    /// The kit recovery (Milestone 7), while one runs.
    recovery: Option<recovery::RecoverySession>,
    /// The export session (ADR-0036), while one runs. Sessions live in memory only and go with
    /// the core when the app locks (`Core::lock` consumes it).
    export: Option<export::ExportSession>,
    /// The id of the last export session handed out (ids count up from 1, never reused while
    /// the core runs).
    export_counter: u32,
    /// The import session (ADR-0036), while one runs.
    import: Option<import::ImportSession>,
    /// The id of the last import session handed out.
    import_counter: u32,
    ai: ai::AiState,
    /// Shared spaces (Milestone 13): the membership timelines and the co-members' device logs.
    shared: membership::SharedRuntime,
    /// The invite session (Milestone 13), while one runs.
    invite: Option<invites::InviteSession>,
}

impl Core {
    /// Opens (or initialises) the workspace: loads or creates the ARK, derives the DB
    /// key, opens the encrypted database, migrates, ensures device identity, loads
    /// the documents, repairs the derived indexes, verifies the device log and the
    /// keyring and gives every space and document its key record. Fails with
    /// `ErrorCode::Locked` when the app lock is enabled; shells use
    /// [`Core::open_or_locked`] to get the [`LockedCore`] instead.
    pub fn open(deps: CoreDeps) -> Result<Self, ApiError> {
        match Self::open_or_locked(deps)? {
            Opened::Ready(core) => Ok(core),
            Opened::Locked(_) => Err(ApiError::locked()),
        }
    }

    /// Executes one request.
    pub fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::AppStatus => Ok(Response::AppStatus(self.app_status()?)),
            Request::WorkspaceCreate {
                name,
                first_space_name,
            } => self.workspace_create(name, first_space_name),
            Request::WorkspaceGet => Ok(Response::Workspace(
                self.db
                    .workspace_get()
                    .map_err(map_storage_error)?
                    .map(workspace_from_row),
            )),
            Request::SettingsGet { key } => {
                validate::setting_key(&key)?;
                let value = self.db.setting_get(&key).map_err(map_storage_error)?;
                Ok(Response::Setting(value))
            }
            Request::SettingsSet { key, value } => {
                validate::setting_key(&key)?;
                validate::setting_value(&value, MAX_SETTING_BYTES)?;
                let now = self.clock.now_ms();
                self.db
                    .setting_set(&key, &value, now)
                    .map_err(map_storage_error)?;
                if key == kb_types::attachments::DOWNLOAD_POLICY_SETTING {
                    // Turning the policy eager wants every missing chunk at once.
                    self.reconcile_chunks()?;
                }
                self.events.push(Event::SettingChanged { key });
                Ok(Response::Ok)
            }
            Request::DeviceInfo => Ok(Response::DeviceInfo(self.device_info())),
            Request::DeviceRename { name } => {
                let name = validate::display_name(&name, 100, "name")?;
                self.db.device_rename(&name).map_err(map_storage_error)?;
                self.device.name.clone_from(&name);
                // The account document carries device names across root key rotations.
                if let Some(account) = self.docs.account.as_ref() {
                    account
                        .model
                        .set_device_name(self.device.device_id, &name)
                        .map_err(documents::map_model_error)?;
                    self.persist_account()?;
                }
                self.events.push(Event::DeviceRenamed { name });
                Ok(Response::DeviceInfo(self.device_info()))
            }
            pages @ (Request::Search { .. }
            | Request::BacklinksGet { .. }
            | Request::PageContentGet { .. }
            | Request::PageFirstImages { .. }
            | Request::PageExcerpts { .. }
            | Request::PageExportMarkdown { .. }
            | Request::IndexRebuild { .. }) => self.dispatch_pages(pages),
            history @ (Request::PageHistoryList { .. }
            | Request::PageHistoryGet { .. }
            | Request::PageHistoryRestore { .. }) => self.dispatch_history(history),
            reminders @ Request::RemindersDue { .. } => self.dispatch_reminders(reminders),
            properties @ (Request::PropertySchemaGet { .. }
            | Request::PropertyCreate { .. }
            | Request::PropertyRename { .. }
            | Request::PropertyConfigSet { .. }
            | Request::PropertyDelete { .. }
            | Request::PropertyMove { .. }
            | Request::PropertyOptionAdd { .. }
            | Request::PropertyOptionUpdate { .. }
            | Request::PropertyOptionDelete { .. }
            | Request::PagePropertySet { .. }
            | Request::PagePropertiesGet { .. }
            | Request::PagesByProperty { .. }) => self.dispatch_properties(properties),
            security @ (Request::SecurityStatus
            | Request::RecoveryKitCreate
            | Request::RecoveryKitVerify { .. }
            | Request::RecoveryKitReissue { .. }
            | Request::AppLockEnable { .. }
            | Request::AppLockDisable { .. }
            | Request::AppLockChange { .. }
            | Request::DeviceList) => self.dispatch_security(security),
            attachments @ (Request::AttachmentBegin { .. }
            | Request::AttachmentFinish { .. }
            | Request::AttachmentAbort { .. }
            | Request::AttachmentList { .. }
            | Request::AttachmentGet { .. }
            | Request::AttachmentDelete { .. }
            | Request::AttachmentGc
            | Request::AttachmentDownload { .. }) => self.dispatch_attachments(attachments),
            pairing @ (Request::PairingStart { .. }
            | Request::PairingConfirm
            | Request::PairingCancel
            | Request::PairingStatus) => self.dispatch_pairing(pairing),
            devices @ (Request::DeviceRevoke { .. } | Request::RotationApply { .. }) => {
                self.dispatch_devices(devices)
            }
            recovery @ (Request::RecoveryStart { .. }
            | Request::RecoveryStatus
            | Request::RecoveryCancel) => self.dispatch_recovery(recovery),
            sync @ (Request::SyncConfigure { .. } | Request::SyncStatus | Request::SyncNow) => {
                self.dispatch_sync(sync)
            }
            export @ (Request::ExportBegin { .. }
            | Request::ExportNext { .. }
            | Request::ExportEnd { .. }
            | Request::ExportCsv { .. }) => self.dispatch_export(export),
            import @ (Request::ImportBegin { .. }
            | Request::ImportBeginSpaces { .. }
            | Request::ImportAddFile { .. }
            | Request::ImportPlan { .. }
            | Request::ImportAddAttachment { .. }
            | Request::ImportCommit { .. }
            | Request::ImportAbort { .. }) => self.dispatch_import(import),
            ai @ (Request::AiIndexStatus
            | Request::AiIndexNext { .. }
            | Request::AiIndexStore { .. }
            | Request::AiIndexClear
            | Request::AiRetrieve { .. }) => self.dispatch_ai(ai),
            mcp @ (Request::McpClientList
            | Request::McpClientCreate { .. }
            | Request::McpClientGrantsSet { .. }
            | Request::McpClientDelete { .. }
            | Request::McpAuditList { .. }
            | Request::McpAuditClear
            | Request::McpCall { .. }) => self.dispatch_mcp(mcp),
            space_move
            @ (Request::PageMoveToSpacePlan { .. } | Request::PageMoveToSpace { .. }) => {
                self.dispatch_space_move(space_move)
            }
            sharing @ (Request::SpaceSharePlan { .. }
            | Request::SpaceShare { .. }
            | Request::SpaceMembers { .. }
            | Request::MemberRoleSet { .. }
            | Request::MemberRemove { .. }
            | Request::SpaceLeave { .. }
            | Request::SpaceClose { .. }
            | Request::SpaceSettingsSet { .. }
            | Request::SpaceAiSet { .. }) => self.dispatch_sharing(sharing),
            profile @ (Request::ProfileGet
            | Request::ProfileSet { .. }
            | Request::PersonGet { .. }
            | Request::PersonVerifiedSet { .. }) => self.dispatch_profile(profile),
            invites @ (Request::InviteLinkCreate { .. }
            | Request::InviteRevoke { .. }
            | Request::InviteCodeStart { .. }
            | Request::InviteCodeJoin { .. }
            | Request::InviteLinkOpen { .. }
            | Request::InviteConfirm
            | Request::InviteCancel
            | Request::InviteStatus) => self.dispatch_invites(invites),
            comments @ (Request::CommentList { .. }
            | Request::CommentPost { .. }
            | Request::CommentEdit { .. }
            | Request::CommentDelete { .. }
            | Request::CommentResolve { .. }) => self.dispatch_comments(comments),
            notifications @ (Request::NotificationList { .. }
            | Request::NotificationsMarkRead { .. }) => self.dispatch_notifications(notifications),
            mentions @ Request::PeopleMentionable { .. } => self.dispatch_mentions(mentions),
            presence @ Request::PresenceList { .. } => self.dispatch_presence(presence),
            links @ (Request::ViewLinkCreate { .. }
            | Request::ViewLinkList { .. }
            | Request::ViewLinkRevoke { .. }) => self.dispatch_links(links),
            structure => self.dispatch_structure(structure),
        }
    }

    /// Returns and clears the events emitted since the last call.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn app_status(&self) -> Result<AppStatus, ApiError> {
        let workspace_initialized = self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some();
        Ok(AppStatus {
            workspace_initialized,
            schema_version: self.db.schema_version().map_err(map_storage_error)?,
            db_encrypted: self.db.encryption().is_encrypted(),
            db_cipher: self.db.encryption().describe(),
            device_id: self.device.device_id,
            data_dir: self.data_dir.clone(),
            core_version: CORE_VERSION.to_owned(),
            api_version: API_VERSION,
            platform: self.platform.clone(),
        })
    }

    fn workspace_create(
        &mut self,
        name: String,
        first_space_name: Option<String>,
    ) -> Result<Response, ApiError> {
        let name = validate::display_name(&name, 200, "name")?;
        let first_space = first_space_name
            .map(|n| validate::display_name(&n, 100, "firstSpaceName"))
            .transpose()?;
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some()
        {
            return Err(ApiError::already_exists("a workspace already exists"));
        }
        let row = WorkspaceRow {
            id: WorkspaceId::generate(self.rng.as_ref()),
            name,
            created_at: self.clock.now_ms(),
        };
        let inserted = self.db.workspace_create(&row).map_err(map_storage_error)?;
        if !inserted {
            return Err(ApiError::already_exists("a workspace already exists"));
        }
        self.events.push(Event::WorkspaceCreated { id: row.id });
        self.ensure_account()?;
        if let Some(space_name) = first_space {
            self.space_create(&space_name, None)?;
        }
        Ok(Response::Workspace(Some(workspace_from_row(row))))
    }

    fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            device_id: self.device.device_id,
            name: self.device.name.clone(),
            platform: self.platform.clone(),
            created_at: self.device.created_at,
        }
    }
}

pub(crate) fn default_device_name(platform: &str) -> String {
    let platform = match platform {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        "android" => "Android",
        "web" => "Web",
        other => other,
    };
    format!("{platform} device")
}

fn workspace_from_row(row: WorkspaceRow) -> Workspace {
    Workspace {
        id: row.id,
        name: row.name,
        created_at: row.created_at,
    }
}

pub(crate) use documents::map_doc_error;

/// The first 8 characters of an id as text: the only form of an id that may appear in a log (M13: logs carry ids cut to 8 hex
/// characters, never a full space, page, document or device id).
pub(crate) fn short_id(id: &impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect()
}

pub(crate) fn map_storage_error(err: StorageError) -> ApiError {
    match err {
        StorageError::WrongKey => ApiError::crypto("the database key was rejected"),
        StorageError::Migration(msg) => {
            ApiError::storage(format!("schema migration failed: {msg}"))
        }
        StorageError::InvalidData(msg) => ApiError::storage(format!("invalid stored data: {msg}")),
        StorageError::Open(msg) => ApiError::storage(format!("cannot open database: {msg}")),
        StorageError::Sqlite(e) => ApiError::storage(format!("database error: {e}")),
        StorageError::Blob(msg) => ApiError::storage(format!("blob store error: {msg}")),
    }
}

pub(crate) fn map_keys_error(err: KeysError) -> ApiError {
    match err {
        KeysError::Keystore(KeystoreError::Unavailable(msg)) => {
            ApiError::keystore_unavailable(format!("no usable keystore: {msg}"))
        }
        KeysError::Keystore(KeystoreError::Other(msg)) => {
            ApiError::internal(format!("keystore error: {msg}"))
        }
        KeysError::Corrupted(msg) => ApiError::crypto(format!("key material corrupted: {msg}")),
        KeysError::Crypto(e) => ApiError::crypto(format!("cryptographic failure: {e}")),
        KeysError::Log(e) => ApiError::crypto(format!("the device log does not verify: {e}")),
        KeysError::InvalidKit(msg) => ApiError::invalid("kit", msg),
        KeysError::WrongPassphrase => ApiError::invalid("passphrase", "wrong passphrase"),
        KeysError::PassphraseLength => ApiError::invalid("passphrase", err.to_string()),
        KeysError::Locked => ApiError::locked(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, create_workspace, status};
    use kb_keys::KeystoreError;
    use kb_types::{ErrorCode, Json};

    #[test]
    fn open_creates_device_identity_and_reopen_keeps_it() {
        let fx = Fixture::new();
        let mut core = fx.open(1, 1_000);
        let first = match core.dispatch(Request::DeviceInfo).unwrap() {
            Response::DeviceInfo(d) => d,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(first.name, "Windows device");
        assert_eq!(first.created_at, 1_000);
        assert_eq!(first.platform, "windows");
        drop(core);

        let mut core = fx.open(2, 9_999);
        let second = match core.dispatch(Request::DeviceInfo).unwrap() {
            Response::DeviceInfo(d) => d,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            second.device_id, first.device_id,
            "identity survives reopen"
        );
        assert_eq!(second.created_at, 1_000);
        assert_eq!(
            fx.keystore
                .get(&kb_keys::ark_entry_name(&fx.install_id))
                .unwrap()
                .map(|v| v.len()),
            Some(32)
        );
    }

    #[test]
    fn app_status_reports_schema_and_workspace_state() {
        let fx = Fixture::new();
        let mut core = fx.open(1, 0);
        let s = status(&mut core);
        assert!(!s.workspace_initialized);
        assert_eq!(s.schema_version, kb_storage::SCHEMA_VERSION);
        assert!(!s.db_encrypted, "plain test opener");
        assert_eq!(s.db_cipher, "none");
        assert_eq!(s.api_version, API_VERSION);
        assert_eq!(s.core_version, CORE_VERSION);
        assert_eq!(s.platform, "windows");
    }

    #[test]
    fn workspace_create_get_and_duplicate() {
        let fx = Fixture::new();
        let mut core = fx.open(1, 42);
        assert_eq!(
            core.dispatch(Request::WorkspaceGet).unwrap(),
            Response::Workspace(None)
        );

        let ws = create_workspace(&mut core, "  Home  ", None);
        assert_eq!(ws.name, "Home", "name is trimmed");
        assert_eq!(ws.created_at, 42);
        assert_eq!(
            core.drain_events(),
            vec![Event::WorkspaceCreated { id: ws.id }]
        );
        assert!(core.drain_events().is_empty(), "drain clears");

        assert_eq!(
            core.dispatch(Request::WorkspaceGet).unwrap(),
            Response::Workspace(Some(ws.clone()))
        );
        let dup = core
            .dispatch(Request::WorkspaceCreate {
                name: "Other".into(),
                first_space_name: None,
            })
            .unwrap_err();
        assert_eq!(dup.code, ErrorCode::AlreadyExists);
        assert!(status(&mut core).workspace_initialized);
    }

    #[test]
    fn workspace_name_validation() {
        let fx = Fixture::new();
        let mut core = fx.open(1, 0);
        let too_long = "x".repeat(201);
        for bad in ["", "   ", too_long.as_str(), "bad\u{0}name"] {
            let err = core
                .dispatch(Request::WorkspaceCreate {
                    name: bad.into(),
                    first_space_name: None,
                })
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
            assert_eq!(err.details, Some(serde_json::json!({ "field": "name" })));
        }
        assert!(core.drain_events().is_empty());
    }

    #[test]
    fn settings_roundtrip_validation_and_events() {
        let fx = Fixture::new();
        let mut core = fx.open(1, 5);
        assert_eq!(
            core.dispatch(Request::SettingsGet {
                key: "ui.theme".into()
            })
            .unwrap(),
            Response::Setting(None)
        );
        let value: Json = serde_json::json!({ "mode": "dark", "accent": [1, 2, 3] });
        assert_eq!(
            core.dispatch(Request::SettingsSet {
                key: "ui.theme".into(),
                value: value.clone()
            })
            .unwrap(),
            Response::Ok
        );
        assert_eq!(
            core.dispatch(Request::SettingsGet {
                key: "ui.theme".into()
            })
            .unwrap(),
            Response::Setting(Some(value))
        );
        assert_eq!(
            core.drain_events(),
            vec![Event::SettingChanged {
                key: "ui.theme".into()
            }]
        );

        let too_long_key = format!("ui.{}", "k".repeat(130));
        for bad_key in [
            "",
            "theme",
            "UI.theme",
            "ui.",
            "ui..x",
            "other.thing",
            "ui.has space",
            too_long_key.as_str(),
        ] {
            let err = core
                .dispatch(Request::SettingsGet {
                    key: bad_key.into(),
                })
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{bad_key:?}");
        }
        let huge = Json::String("v".repeat(MAX_SETTING_BYTES + 1));
        let err = core
            .dispatch(Request::SettingsSet {
                key: "app.blob".into(),
                value: huge,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert!(core.drain_events().is_empty());
    }

    #[test]
    fn device_rename_persists_and_validates() {
        let fx = Fixture::new();
        let mut core = fx.open(1, 0);
        let renamed = core
            .dispatch(Request::DeviceRename {
                name: " Ben's laptop ".into(),
            })
            .unwrap();
        match renamed {
            Response::DeviceInfo(d) => assert_eq!(d.name, "Ben's laptop"),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            core.drain_events(),
            vec![Event::DeviceRenamed {
                name: "Ben's laptop".into()
            }]
        );
        let err = core
            .dispatch(Request::DeviceRename {
                name: "x".repeat(101),
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        drop(core);

        let mut core = fx.open(3, 1);
        match core.dispatch(Request::DeviceInfo).unwrap() {
            Response::DeviceInfo(d) => assert_eq!(d.name, "Ben's laptop"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn keystore_unavailable_maps_to_its_error_code() {
        struct Broken;
        impl Keystore for Broken {
            fn get(&self, _: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, KeystoreError> {
                Err(KeystoreError::Unavailable("no secret service".into()))
            }
            fn set(&self, _: &str, _: &[u8]) -> Result<(), KeystoreError> {
                Err(KeystoreError::Unavailable("no secret service".into()))
            }
            fn delete(&self, _: &str) -> Result<(), KeystoreError> {
                Ok(())
            }
        }
        let fx = Fixture::new();
        let mut deps = fx.deps(1, 0);
        deps.keystore = Box::new(Broken);
        let err = Core::open(deps).err().expect("open must fail");
        assert_eq!(err.code, ErrorCode::KeystoreUnavailable);
    }
}
