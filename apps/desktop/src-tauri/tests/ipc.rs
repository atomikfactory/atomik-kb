//! The shell through Tauri's real IPC layer, on the mock runtime: the commands are
//! registered under the documented names, extract their arguments the way the webview
//! sends them, reject bad input with the `ApiError` JSON, and the main window has no more
//! permissions than `core:default`.
//!
//! Two kinds of state sit behind the commands. The real core needs a keystore, so a test
//! that only checks registration and argument handling leaves it opening (what reaches it
//! is observable as `Internal: "core is starting"`). The app lock and the backup commands
//! run against a fake core (`Fake`, implementing the shell's `CoreApi`/`LockedApi` seams)
//! so their whole path is exercised: IPC arguments, the state machine, the files on disk.
//! What the real core does with a backup is covered in `kb-core`. The attachment commands
//! run against the same fake, which keeps uploads and finished attachments in memory
//! (`Spy::stored`); what the real core does with them is covered in `kb-core` too.
//!
//! No test here may open a native dialog (or hand a file to the system opener): every
//! backup or attachment call that gets as far as the dialog (a ready core and no `path`) is
//! a bug in the test. `attachment_open` is only tested up to the point where it refuses. The commands check the state first,
//! so a locked or starting core answers before any dialog would open, and the tests that
//! rely on that say so.
//!
//! This is an integration test, not a unit test, on purpose: on Windows a test executable
//! that builds a Tauri app needs the Common Controls 6 manifest, and `build.rs` embeds it
//! into integration tests only (cargo has no such hook for the unit tests of a library).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use atomik_kb_desktop_lib::{
    AppState, AttachmentSink, BackupSink, BackupSource, CoreApi, LockedApi, MAIN_WINDOW,
    SharedState, configure, context, create_main_window,
};
use kb_net::{RealtimeCommand, RealtimeHandle};
use kb_sync::v2::{PresenceFrame, SealedPresence};
use kb_types::attachments::{CHUNK_SIZE, MAX_PREVIEW_BYTES};
use kb_types::{
    AccountId, ApiError, AttachmentId, AttachmentInfo, BackupSummary, ByteRange, DeviceId, Event,
    FeedId, PageId, PresenceInfo, PresenceMessage, PresenceState, Request, Response, SpaceId,
};
use serde_json::{Value, json};
use tauri::http::{HeaderMap, HeaderName, HeaderValue};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{
    INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The header that carries the page id, spelled out here so a rename in the shell breaks
/// this test (and the TypeScript contract test) instead of passing silently.
const PAGE_HEADER: &str = "x-kb-page";

const PAGE: &str = "12345678-9abc-4def-8123-456789abcdef";

const DOC_COMMANDS: [&str; 4] = ["doc_open", "doc_import", "doc_export", "doc_close"];

/// The headers of the raw attachment commands, spelled out here for the same reason.
const ATTACHMENT_HEADER: &str = "x-kb-attachment";
const RANGE_START_HEADER: &str = "x-kb-range-start";
const RANGE_END_HEADER: &str = "x-kb-range-end";

const ATTACHMENT: &str = "abcdefab-cdef-4abc-8def-abcdefabcdef";

/// The space the tests import into.
const SPACE: &str = "09090909-0909-0909-0909-090909090909";

const ATTACHMENT_RAW_COMMANDS: [&str; 2] = ["attachment_write", "attachment_read"];

/// The JSON attachment commands with the arguments that make them pass argument
/// extraction and reach the state check (no `path`: a ready core would open a dialog).
fn attachment_json_commands() -> [(&'static str, Value); 4] {
    [
        (
            "attachment_import_path",
            json!({ "spaceId": SPACE, "path": null }),
        ),
        // A link that would be fetched: a locked or starting core must answer before any
        // request is made.
        (
            "attachment_import_url",
            json!({ "spaceId": SPACE, "url": "https://example.com/a.png" }),
        ),
        (
            "attachment_save_as",
            json!({ "attachmentId": ATTACHMENT, "path": null }),
        ),
        ("attachment_open", json!({ "attachmentId": ATTACHMENT })),
    ]
}

/// A backup is `KBB1` and whatever the fake core streams after it.
const BACKUP_PIECES: [&[u8]; 3] = [b"KBB1", &[1, 2, 3], &[255]];

/// An app with the shell's commands, state and plugins, on the mock runtime and with the
/// real `tauri.conf.json` and capabilities. Its core is the real one, still opening.
fn app() -> App<MockRuntime> {
    let state: SharedState = Arc::new(AppState::<kb_core::Core>::new());
    configure(mock_builder(), state).build(context()).unwrap()
}

/// The webview the shell's capability is written for.
fn main_webview(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .build()
        .unwrap()
}

/// Sends one IPC message as the webview's `invoke` would, from the app's own origin.
fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    command: &str,
    body: InvokeBody,
    headers: &[(&str, &str)],
) -> Result<InvokeResponseBody, Value> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: (if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" })
                .parse()
                .unwrap(),
            body,
            headers: map,
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
}

/// A JSON command (arguments as the webview's `invoke(command, { ... })` sends them).
fn call(webview: &WebviewWindow<MockRuntime>, command: &str, args: Value) -> Result<Value, Value> {
    invoke(webview, command, InvokeBody::Json(args), &[])
        .map(|body| body.deserialize::<Value>().unwrap())
}

/// What a call that reaches the not-yet-open core is answered with.
fn starting() -> Value {
    json!({ "code": "Internal", "message": "core is starting" })
}

/// What every command but `lock`, `unlock` and `core_state` is answered with while locked.
fn locked() -> Value {
    json!({ "code": "Locked", "message": "the workspace is locked" })
}

// --- the fake core ---

/// What the fake core saw, shared with the test that owns the state.
#[derive(Default)]
struct Spy {
    requests: Vec<&'static str>,
    docs: Vec<&'static str>,
    lock_calls: usize,
    unlock_attempts: Vec<String>,
    exports: usize,
    imports: Vec<Import>,
    /// What the attachment API received, in order: `begin`, `write:<bytes>`, `finish`,
    /// `abort`, `get`, `read:full`, `read:<start>-<end>`.
    attachment_calls: Vec<String>,
    uploads: BTreeMap<AttachmentId, Stored>,
    stored: BTreeMap<AttachmentId, Stored>,
    next_attachment: u8,
    writes: usize,
    /// The 1-based number of the `attachment_write` call that fails with `Storage`.
    fail_write_on: Option<usize>,
    /// What `presence_seal` answers (`None`: nothing to send) and the calls it got, in order.
    sealed: Option<SealedPresence>,
    /// What `presence_seal` fails with, when it does.
    seal_error: Option<ApiError>,
    seals: Vec<(SpaceId, Option<PageId>, PresenceState, Vec<u8>)>,
    /// What `attachment_read` fails with after its first piece.
    fail_read: Option<ApiError>,
}

/// An attachment of the fake core, finished or still uploading.
#[derive(Clone)]
struct Stored {
    space_id: SpaceId,
    name: String,
    mime: String,
    bytes: Vec<u8>,
    /// What the info says the size is, when the test cannot afford the real bytes.
    claimed_size: Option<u64>,
}

fn info_of(attachment_id: AttachmentId, stored: &Stored) -> AttachmentInfo {
    let size = stored.claimed_size.unwrap_or(stored.bytes.len() as u64);
    AttachmentInfo {
        attachment_id,
        space_id: stored.space_id,
        name: stored.name.clone(),
        mime: stored.mime.clone(),
        size,
        chunk_count: u32::try_from(size.div_ceil(CHUNK_SIZE as u64).max(1)).unwrap(),
        created_at: 1_700_000_000_000,
        references: 0,
        unreferenced_since: None,
        local_chunks: u32::try_from(size.div_ceil(CHUNK_SIZE as u64).max(1)).unwrap_or(u32::MAX),
        availability: kb_types::AttachmentAvailability::Complete,
    }
}

/// One `backup_import` call: everything the fake read from the source and the kit it got.
#[derive(Debug, PartialEq)]
struct Import {
    bytes: Vec<u8>,
    kit: Option<String>,
}

type SharedSpy = Arc<Mutex<Spy>>;

/// A core that records what it is asked, streams [`BACKUP_PIECES`] on export and has an
/// app lock whose passphrase is `passphrase`.
struct Fake {
    spy: SharedSpy,
    passphrase: &'static str,
    /// What `lock` refuses with, when it does.
    lock_refusal: Option<ApiError>,
    /// What `backup_export` and `backup_import` fail with, after doing their streaming.
    backup_failure: Option<ApiError>,
}

impl Fake {
    fn new() -> (Self, SharedSpy) {
        let spy = SharedSpy::default();
        let fake = Self {
            spy: Arc::clone(&spy),
            passphrase: "open sesame",
            lock_refusal: None,
            backup_failure: None,
        };
        (fake, spy)
    }
}

struct FakeLocked(Fake);

impl LockedApi for FakeLocked {
    type Core = Fake;

    fn unlock(self, passphrase: &str) -> Result<Fake, (Self, ApiError)> {
        self.0
            .spy
            .lock()
            .unwrap()
            .unlock_attempts
            .push(passphrase.to_owned());
        if passphrase == self.0.passphrase {
            Ok(self.0)
        } else {
            Err((
                self,
                ApiError::invalid("passphrase", "the passphrase is wrong"),
            ))
        }
    }
}

fn summary(bytes: u64) -> BackupSummary {
    BackupSummary {
        bytes,
        documents: 2,
        settings: 3,
        created_at: 1_700_000_000_000,
        account_id: AccountId::from_bytes([7; 16]),
        chunks: 0,
        chunk_bytes: 0,
    }
}

impl CoreApi for Fake {
    type Locked = FakeLocked;

    fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
        let mut spy = self.spy.lock().unwrap();
        match request {
            Request::AttachmentBegin {
                space_id,
                name,
                mime,
            } => {
                spy.attachment_calls.push("begin".into());
                spy.next_attachment += 1;
                let id = AttachmentId::from_bytes([spy.next_attachment; 16]);
                let upload = Stored {
                    space_id,
                    name,
                    mime,
                    bytes: Vec::new(),
                    claimed_size: None,
                };
                spy.uploads.insert(id, upload);
                Ok(Response::AttachmentStarted(id))
            }
            Request::AttachmentFinish { attachment_id } => {
                spy.attachment_calls.push("finish".into());
                let upload = spy
                    .uploads
                    .remove(&attachment_id)
                    .ok_or_else(|| ApiError::not_found("no such upload"))?;
                let info = info_of(attachment_id, &upload);
                spy.stored.insert(attachment_id, upload);
                Ok(Response::Attachment(info))
            }
            Request::AttachmentAbort { attachment_id } => {
                spy.attachment_calls.push("abort".into());
                spy.uploads.remove(&attachment_id);
                Ok(Response::Ok)
            }
            Request::AttachmentGet { attachment_id } => {
                spy.attachment_calls.push("get".into());
                spy.stored
                    .get(&attachment_id)
                    .map(|stored| Response::Attachment(info_of(attachment_id, stored)))
                    .ok_or_else(|| ApiError::not_found("no such attachment"))
            }
            other => {
                spy.requests.push(other.kind());
                Ok(Response::Ok)
            }
        }
    }

    fn presence_seal(
        &mut self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> Result<Option<SealedPresence>, ApiError> {
        let mut spy = self.spy.lock().unwrap();
        spy.seals.push((space_id, page_id, state, update.to_vec()));
        match spy.seal_error.clone() {
            Some(error) => Err(error),
            None => Ok(spy.sealed.clone()),
        }
    }

    fn drain_events(&mut self) -> Vec<Event> {
        Vec::new()
    }

    fn page_doc_open(&mut self, _: PageId) -> Result<Vec<u8>, ApiError> {
        self.spy.lock().unwrap().docs.push("open");
        Ok(vec![1])
    }

    fn page_doc_import(&mut self, _: PageId, _: &[u8]) -> Result<(), ApiError> {
        self.spy.lock().unwrap().docs.push("import");
        Ok(())
    }

    fn page_doc_export(&mut self, _: PageId, _: Option<&[u8]>) -> Result<Vec<u8>, ApiError> {
        self.spy.lock().unwrap().docs.push("export");
        Ok(vec![2])
    }

    fn page_doc_close(&mut self, _: PageId) -> Result<(), ApiError> {
        self.spy.lock().unwrap().docs.push("close");
        Ok(())
    }

    fn lock(self) -> Result<FakeLocked, (Self, ApiError)> {
        self.spy.lock().unwrap().lock_calls += 1;
        match self.lock_refusal.clone() {
            Some(error) => Err((self, error)),
            None => Ok(FakeLocked(self)),
        }
    }

    fn backup_export(&mut self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
        self.spy.lock().unwrap().exports += 1;
        let mut bytes = 0;
        for piece in BACKUP_PIECES {
            sink(piece).map_err(ApiError::storage)?;
            bytes += piece.len() as u64;
        }
        match &self.backup_failure {
            Some(error) => Err(error.clone()),
            None => Ok(summary(bytes)),
        }
    }

    fn backup_import(
        &mut self,
        source: BackupSource<'_>,
        kit: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 3];
        loop {
            let count = source(&mut buffer).map_err(ApiError::storage)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let size = bytes.len() as u64;
        self.spy.lock().unwrap().imports.push(Import {
            bytes,
            kit: kit.map(str::to_owned),
        });
        match &self.backup_failure {
            Some(error) => Err(error.clone()),
            None => Ok(summary(size)),
        }
    }

    fn attachment_write(
        &mut self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let mut spy = self.spy.lock().unwrap();
        spy.attachment_calls.push(format!("write:{}", bytes.len()));
        spy.writes += 1;
        if spy.fail_write_on == Some(spy.writes) {
            return Err(ApiError::storage("the chunk could not be stored"));
        }
        spy.uploads
            .get_mut(&attachment_id)
            .ok_or_else(|| ApiError::not_found("no such upload"))?
            .bytes
            .extend_from_slice(bytes);
        Ok(())
    }

    fn attachment_read(
        &mut self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        let (stored, fail) = {
            let mut spy = self.spy.lock().unwrap();
            spy.attachment_calls.push(match range {
                None => "read:full".to_owned(),
                Some(range) => format!("read:{}-{}", range.start, range.end),
            });
            let stored = spy
                .stored
                .get(&attachment_id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("no such attachment"))?;
            (stored, spy.fail_read.clone())
        };
        let wanted = match range {
            None => &stored.bytes[..],
            Some(range) => {
                let start = usize::try_from(range.start).unwrap();
                let end = usize::try_from(range.end).unwrap();
                if end > stored.bytes.len() {
                    return Err(ApiError::invalid("range", "the range is outside the file"));
                }
                &stored.bytes[start..end]
            }
        };
        // Pieces of 5 bytes: the shell must cope with any size.
        for (index, piece) in wanted.chunks(5).enumerate() {
            sink(piece)?;
            if index == 0
                && let Some(error) = fail.clone()
            {
                return Err(error);
            }
        }
        Ok(info_of(attachment_id, &stored))
    }
}

/// An app whose core is `fake`, already open (a ready core), and the state to reach into.
fn fake_app(fake: Fake) -> (App<MockRuntime>, Arc<AppState<Fake>>) {
    let state = Arc::new(AppState::<Fake>::new());
    state.set_ready(fake);
    let shared: SharedState = Arc::clone(&state) as SharedState;
    let app = configure(mock_builder(), shared).build(context()).unwrap();
    (app, state)
}

/// `core_state` as the webview sees it.
fn core_state(webview: &WebviewWindow<MockRuntime>) -> Value {
    call(webview, "core_state", json!({})).unwrap()
}

/// The text of a path as the IPC carries it.
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// The names of the files in `dir`, sorted.
fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn backup_bytes() -> Vec<u8> {
    BACKUP_PIECES.concat()
}

// --- registration ---

#[test]
fn every_documented_command_is_registered() {
    let app = app();
    let webview = main_webview(&app);
    // `backup_export` and `backup_import` get an empty body, i.e. no `path`: with a ready
    // core that would open a native dialog. The core is still opening here, so they answer
    // before they get that far (see `a_locked_core_...` for the same promise when locked).
    for command in [
        "core_state",
        "dispatch",
        "subscribe_events",
        "doc_open",
        "doc_import",
        "doc_export",
        "doc_close",
        "open_external",
        "lock",
        "unlock",
        "backup_export",
        "backup_import",
        "attachment_write",
        "attachment_read",
        "attachment_import_path",
        "attachment_import_url",
        "attachment_save_as",
        "attachment_open",
        "export_run",
        "import_run",
        "save_text_file",
        "backup_folder_choose",
        "backup_run_now",
        "ai_status",
        "ai_endpoint_set",
        "ai_generate",
        "ai_chat",
        "ai_cancel",
        "ai_search",
        "ai_transcribe",
        "mcp_status",
        "mcp_client_grants_set",
        "shell_events_subscribe",
        "shell_labels_set",
        "menu_bar_show",
        "menu_bar_hide",
        "presence_send",
        "presence_events",
        "dictation_status",
        "dictation_select",
        "dictation_install",
        "dictation_install_cancel",
        "dictation_import",
        "dictation_remove",
        "dictation_start",
        "dictation_audio",
        "dictation_stop",
        "dictation_cancel",
        "drive_status",
        "drive_connect",
        "drive_disconnect",
        "drive_client_set",
        "drive_backups_list",
        "drive_backup_download",
        "drive_cancel",
        "backup_folder_list",
    ] {
        let message = match invoke(&webview, command, InvokeBody::default(), &[]) {
            Ok(_) => String::new(),
            Err(error) => error.to_string(),
        };
        assert!(
            !message.contains("not found"),
            "{command} is not registered: {message}"
        );
    }
}

#[test]
fn a_command_the_shell_does_not_have_is_rejected() {
    let app = app();
    let webview = main_webview(&app);
    let error = invoke(&webview, "doc_delete", InvokeBody::default(), &[]).unwrap_err();
    assert_eq!(error, json!("Command doc_delete not found"));
}

#[test]
fn the_json_commands_keep_working() {
    let app = app();
    let webview = main_webview(&app);

    let state = invoke(&webview, "core_state", InvokeBody::default(), &[]).unwrap();
    assert_eq!(state.deserialize::<Value>().unwrap(), json!("opening"));

    let body = InvokeBody::Json(json!({ "request": { "type": "AppStatus" } }));
    assert_eq!(
        invoke(&webview, "dispatch", body, &[]).unwrap_err(),
        starting()
    );
}

// --- raw page-document commands ---

#[test]
fn the_page_document_commands_need_the_page_header() {
    let app = app();
    let webview = main_webview(&app);
    for command in DOC_COMMANDS {
        let error = invoke(&webview, command, InvokeBody::Raw(vec![1, 2, 3]), &[]).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{command}");
        assert_eq!(error["details"]["field"], PAGE_HEADER, "{command}");
    }
}

#[test]
fn a_malformed_page_header_is_rejected_by_every_page_document_command() {
    let app = app();
    let webview = main_webview(&app);
    for command in DOC_COMMANDS {
        let error = invoke(
            &webview,
            command,
            InvokeBody::Raw(vec![1, 2, 3]),
            &[(PAGE_HEADER, "not-a-page-id")],
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{command}");
        assert_eq!(error["details"]["field"], PAGE_HEADER, "{command}");
    }
}

#[test]
fn a_valid_page_call_reaches_the_core() {
    let app = app();
    let webview = main_webview(&app);
    for command in DOC_COMMANDS {
        let error = invoke(
            &webview,
            command,
            InvokeBody::Raw(vec![1, 2, 3]),
            &[(PAGE_HEADER, PAGE)],
        )
        .unwrap_err();
        assert_eq!(error, starting(), "{command}");
    }
}

#[test]
fn every_body_shape_the_transport_can_send_reaches_the_core() {
    let app = app();
    let webview = main_webview(&app);
    let headers = [(PAGE_HEADER, PAGE)];
    // doc_open and doc_close ignore their body; doc_export takes bytes or nothing.
    for body in [
        InvokeBody::Raw(Vec::new()),
        InvokeBody::Raw(vec![0, 255]),
        InvokeBody::Json(json!({})),
        InvokeBody::Json(Value::Null),
        // what the postMessage fallback makes of a Uint8Array
        InvokeBody::Json(json!([0, 255])),
    ] {
        for command in ["doc_open", "doc_export", "doc_close"] {
            let error = invoke(&webview, command, body.clone(), &headers).unwrap_err();
            assert_eq!(error, starting(), "{command} with {body:?}");
        }
    }
}

#[test]
fn doc_import_refuses_an_empty_update_before_it_asks_the_core() {
    let app = app();
    let webview = main_webview(&app);
    for body in [
        InvokeBody::Raw(Vec::new()),
        InvokeBody::Json(json!({})),
        InvokeBody::Json(Value::Null),
        InvokeBody::Json(json!([])),
    ] {
        let error =
            invoke(&webview, "doc_import", body.clone(), &[(PAGE_HEADER, PAGE)]).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{body:?}");
        assert_eq!(error["details"]["field"], "body", "{body:?}");
    }
}

#[test]
fn a_body_that_is_not_bytes_is_refused_where_it_matters() {
    let app = app();
    let webview = main_webview(&app);
    // doc_open and doc_close never look at their body.
    for command in ["doc_import", "doc_export"] {
        for body in [json!({ "update": [1, 2] }), json!("bytes"), json!([1, 300])] {
            let error = invoke(
                &webview,
                command,
                InvokeBody::Json(body.clone()),
                &[(PAGE_HEADER, PAGE)],
            )
            .unwrap_err();
            assert_eq!(error["code"], "InvalidInput", "{command} with {body}");
            assert_eq!(error["details"]["field"], "body", "{command} with {body}");
        }
    }
}

// --- open_external ---

#[test]
fn open_external_refuses_every_scheme_but_web_and_mail() {
    let app = app();
    let webview = main_webview(&app);
    for url in [
        "javascript:alert(1)",
        "file:///C:/Windows/System32/calc.exe",
        "data:text/html,hi",
        "tauri://localhost/",
        "ms-msdt:/id PCWDiagnostic",
        "not a url",
        "",
    ] {
        let error = invoke(
            &webview,
            "open_external",
            InvokeBody::Json(json!({ "url": url })),
            &[],
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{url:?}");
        assert_eq!(error["details"]["field"], "url", "{url:?}");
    }
}

#[test]
fn open_external_needs_its_url_argument() {
    let app = app();
    let webview = main_webview(&app);
    let error = invoke(&webview, "open_external", InvokeBody::Json(json!({})), &[]).unwrap_err();
    let message = error.as_str().unwrap();
    assert!(message.contains("url"), "{message}");
}

// --- core_state and the app lock ---

#[test]
fn core_state_reports_every_state_the_core_can_be_in() {
    let state = Arc::new(AppState::<Fake>::new());
    let shared: SharedState = Arc::clone(&state) as SharedState;
    let app = configure(mock_builder(), shared).build(context()).unwrap();
    let webview = main_webview(&app);

    assert_eq!(core_state(&webview), json!("opening"));

    let (fake, _spy) = Fake::new();
    state.set_ready(fake);
    assert_eq!(core_state(&webview), json!("ready"));

    call(&webview, "lock", json!({})).unwrap();
    assert_eq!(core_state(&webview), json!("locked"));

    state.set_failed(ApiError::keystore_unavailable("no keystore"));
    assert_eq!(
        core_state(&webview),
        json!({ "failed": { "code": "KeystoreUnavailable", "message": "no keystore" } })
    );
}

#[test]
fn a_workspace_that_opens_locked_says_so_and_waits_for_the_passphrase() {
    let state = Arc::new(AppState::<Fake>::new());
    let (fake, spy) = Fake::new();
    state.set_locked(FakeLocked(fake));
    let shared: SharedState = Arc::clone(&state) as SharedState;
    let app = configure(mock_builder(), shared).build(context()).unwrap();
    let webview = main_webview(&app);

    assert_eq!(core_state(&webview), json!("locked"));
    assert_eq!(
        call(
            &webview,
            "dispatch",
            json!({ "request": { "type": "AppStatus" } })
        )
        .unwrap_err(),
        locked()
    );

    call(&webview, "unlock", json!({ "passphrase": "open sesame" })).unwrap();
    assert_eq!(core_state(&webview), json!("ready"));
    assert_eq!(spy.lock().unwrap().unlock_attempts, ["open sesame"]);
}

#[test]
fn lock_and_unlock_move_a_ready_core_through_locked_and_back() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let status = json!({ "request": { "type": "AppStatus" } });

    assert_eq!(
        call(&webview, "dispatch", status.clone()).unwrap(),
        json!({ "type": "Ok" })
    );
    assert_eq!(call(&webview, "lock", json!({})).unwrap(), Value::Null);
    assert_eq!(core_state(&webview), json!("locked"));
    assert_eq!(
        call(&webview, "dispatch", status.clone()).unwrap_err(),
        locked()
    );

    assert_eq!(
        call(&webview, "unlock", json!({ "passphrase": "open sesame" })).unwrap(),
        Value::Null
    );
    assert_eq!(core_state(&webview), json!("ready"));
    assert_eq!(
        call(&webview, "dispatch", status).unwrap(),
        json!({ "type": "Ok" })
    );

    let spy = spy.lock().unwrap();
    assert_eq!(spy.lock_calls, 1);
    // Both dispatches reached the core; the one while locked did not.
    assert_eq!(spy.requests, ["AppStatus", "AppStatus"]);
}

#[test]
fn a_wrong_passphrase_is_invalid_input_for_the_passphrase_and_keeps_the_lock() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();

    let error = call(&webview, "unlock", json!({ "passphrase": "not the one" })).unwrap_err();

    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "passphrase");
    assert_eq!(core_state(&webview), json!("locked"));
    assert_eq!(spy.lock().unwrap().unlock_attempts, ["not the one"]);
    // The right one still works afterwards.
    call(&webview, "unlock", json!({ "passphrase": "open sesame" })).unwrap();
    assert_eq!(core_state(&webview), json!("ready"));
}

#[test]
fn unlock_on_a_core_that_is_not_locked_is_invalid_input_and_does_not_try_the_passphrase() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    let error = call(&webview, "unlock", json!({ "passphrase": "open sesame" })).unwrap_err();

    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["message"], "the workspace is not locked");
    assert!(error.get("details").is_none(), "{error}");
    assert_eq!(core_state(&webview), json!("ready"));
    assert!(spy.lock().unwrap().unlock_attempts.is_empty());
}

#[test]
fn unlock_while_the_core_is_opening_is_answered_like_any_command() {
    let app = app();
    let webview = main_webview(&app);
    assert_eq!(
        call(&webview, "unlock", json!({ "passphrase": "open sesame" })).unwrap_err(),
        starting()
    );
    assert_eq!(call(&webview, "lock", json!({})).unwrap_err(), starting());
}

#[test]
fn a_core_that_refuses_to_lock_keeps_serving_and_its_error_reaches_the_webview() {
    let (mut fake, _spy) = Fake::new();
    fake.lock_refusal = Some(ApiError::invalid("appLock", "the app lock is not enabled"));
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    let error = call(&webview, "lock", json!({})).unwrap_err();

    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "appLock");
    assert_eq!(core_state(&webview), json!("ready"));
    assert_eq!(
        call(
            &webview,
            "dispatch",
            json!({ "request": { "type": "AppStatus" } })
        )
        .unwrap(),
        json!({ "type": "Ok" })
    );
}

#[test]
fn locking_twice_is_not_an_error() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();
    assert_eq!(call(&webview, "lock", json!({})).unwrap(), Value::Null);
    assert_eq!(core_state(&webview), json!("locked"));
    assert_eq!(spy.lock().unwrap().lock_calls, 1);
}

#[test]
fn unlock_needs_its_passphrase_as_a_string() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();

    for args in [
        json!({}),
        json!({ "passphrase": 12345 }),
        json!({ "passphrase": null }),
    ] {
        let error = call(&webview, "unlock", args.clone()).unwrap_err();
        let message = error.as_str().unwrap();
        assert!(message.contains("passphrase"), "{args}: {message}");
    }
    assert!(spy.lock().unwrap().unlock_attempts.is_empty());
    assert_eq!(core_state(&webview), json!("locked"));
}

#[test]
fn every_command_but_the_lock_ones_answers_locked_while_locked() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("backup.atomikkb");
    let import = dir.path().join("existing.atomikkb");
    fs::write(&import, b"KBB1 an existing backup").unwrap();

    assert_eq!(
        call(
            &webview,
            "dispatch",
            json!({ "request": { "type": "AppStatus" } })
        )
        .unwrap_err(),
        locked()
    );
    for command in DOC_COMMANDS {
        let error = invoke(
            &webview,
            command,
            InvokeBody::Raw(vec![1, 2, 3]),
            &[(PAGE_HEADER, PAGE)],
        )
        .unwrap_err();
        assert_eq!(error, locked(), "{command}");
    }
    assert_eq!(
        call(&webview, "backup_export", json!({ "path": text(&export) })).unwrap_err(),
        locked()
    );
    assert_eq!(
        call(
            &webview,
            "backup_import",
            json!({ "path": text(&import), "kit": "legal winner" })
        )
        .unwrap_err(),
        locked()
    );
    for command in ATTACHMENT_RAW_COMMANDS {
        let error = invoke(
            &webview,
            command,
            InvokeBody::Raw(vec![1, 2, 3]),
            &[(ATTACHMENT_HEADER, ATTACHMENT)],
        )
        .unwrap_err();
        assert_eq!(error, locked(), "{command}");
    }
    let source = dir.path().join("source.bin");
    fs::write(&source, b"plaintext").unwrap();
    let saved = dir.path().join("saved.bin");
    for (command, args) in [
        (
            "attachment_import_path",
            json!({ "spaceId": SPACE, "path": text(&source) }),
        ),
        (
            "attachment_save_as",
            json!({ "attachmentId": ATTACHMENT, "path": text(&saved) }),
        ),
        ("attachment_open", json!({ "attachmentId": ATTACHMENT })),
    ] {
        assert_eq!(
            call(&webview, command, args).unwrap_err(),
            locked(),
            "{command}"
        );
    }

    // No file was made, none was read, the core saw nothing.
    assert_eq!(
        files_in(dir.path()),
        ["existing.atomikkb", "source.bin"],
        "a file was made while locked"
    );
    let spy = spy.lock().unwrap();
    assert!(spy.requests.is_empty() && spy.docs.is_empty());
    assert!(spy.attachment_calls.is_empty());
    assert_eq!(spy.exports, 0);
    assert!(spy.imports.is_empty());
    drop(spy);
    assert_eq!(core_state(&webview), json!("locked"));
}

#[test]
fn a_locked_or_starting_core_answers_before_any_dialog_would_open() {
    // No `path`: with a ready core these calls open a native dialog and wait for a person.
    // Reaching the answers below is the proof that they check the state first.
    let (fake, _spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();
    for command in ["backup_export", "backup_import"] {
        assert_eq!(
            call(&webview, command, json!({})).unwrap_err(),
            locked(),
            "{command}"
        );
        assert_eq!(
            call(&webview, command, json!({ "path": null, "kit": null })).unwrap_err(),
            locked(),
            "{command}"
        );
    }

    // The same for the attachment commands that would open a dialog.
    for (command, args) in attachment_json_commands() {
        assert_eq!(
            call(&webview, command, args.clone()).unwrap_err(),
            locked(),
            "{command}"
        );
    }

    let starting_app = self::app();
    let starting_webview = main_webview(&starting_app);
    for command in ["backup_export", "backup_import"] {
        assert_eq!(
            call(&starting_webview, command, json!({})).unwrap_err(),
            starting(),
            "{command}"
        );
    }
    for (command, args) in attachment_json_commands() {
        assert_eq!(
            call(&starting_webview, command, args).unwrap_err(),
            starting(),
            "{command}"
        );
    }
}

// --- backups ---

#[test]
fn backup_export_writes_the_core_s_pieces_to_the_file_and_returns_the_summary_and_path() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("my backup.atomikkb");

    let result = call(&webview, "backup_export", json!({ "path": text(&path) })).unwrap();

    assert_eq!(
        result,
        json!({
            "summary": {
                "bytes": 8,
                "documents": 2,
                "settings": 3,
                "createdAt": 1_700_000_000_000_i64,
                "accountId": "07070707-0707-0707-0707-070707070707",
                "chunks": 0,
                "chunkBytes": 0,
            },
            "path": text(&path),
        })
    );
    assert_eq!(fs::read(&path).unwrap(), backup_bytes());
    // The temporary file is gone: the file is the only thing left in the directory.
    assert_eq!(files_in(dir.path()), ["my backup.atomikkb"]);
    assert_eq!(spy.lock().unwrap().exports, 1, "the core is called once");
}

#[test]
fn backup_export_replaces_a_file_that_is_already_there() {
    let (fake, _spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");
    fs::write(
        &path,
        b"the previous backup, which is longer than the new one by a fair bit",
    )
    .unwrap();

    call(&webview, "backup_export", json!({ "path": text(&path) })).unwrap();

    assert_eq!(fs::read(&path).unwrap(), backup_bytes());
    assert_eq!(files_in(dir.path()), ["backup.atomikkb"]);
}

#[test]
fn a_failed_export_leaves_neither_the_new_file_nor_the_temporary_one() {
    let (mut fake, _spy) = Fake::new();
    fake.backup_failure = Some(ApiError::invalid(
        "workspace",
        "there is no workspace to back up",
    ));
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");

    let error = call(&webview, "backup_export", json!({ "path": text(&path) })).unwrap_err();

    // The core's error, as the webview receives every error.
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "workspace");
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
}

#[test]
fn a_failed_export_keeps_the_previous_backup_at_that_path() {
    let (mut fake, _spy) = Fake::new();
    fake.backup_failure = Some(ApiError::crypto("sealing failed"));
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");
    fs::write(&path, b"the previous backup").unwrap();

    let error = call(&webview, "backup_export", json!({ "path": text(&path) })).unwrap_err();

    assert_eq!(error["code"], "Crypto");
    assert_eq!(fs::read(&path).unwrap(), b"the previous backup");
    assert_eq!(files_in(dir.path()), ["backup.atomikkb"]);
}

#[test]
fn backup_export_refuses_a_bad_path_before_the_core_runs() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let missing_directory = dir.path().join("no-such-directory").join("backup.atomikkb");
    let a_directory = dir.path().to_path_buf();

    for (path, why) in [
        ("backup.atomikkb".to_owned(), "relative"),
        (String::new(), "empty"),
        (text(&missing_directory).to_owned(), "missing directory"),
        (text(&a_directory).to_owned(), "a directory"),
    ] {
        let error = call(&webview, "backup_export", json!({ "path": path })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{why}");
        assert_eq!(error["details"]["field"], "path", "{why}");
    }

    assert_eq!(spy.lock().unwrap().exports, 0);
    assert!(files_in(dir.path()).is_empty());
}

#[test]
fn backup_export_needs_a_string_path() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    let error = call(&webview, "backup_export", json!({ "path": 5 })).unwrap_err();

    let message = error.as_str().unwrap();
    assert!(message.contains("path"), "{message}");
    assert_eq!(spy.lock().unwrap().exports, 0);
}

#[test]
fn backup_import_streams_the_file_into_the_core_and_returns_the_summary_and_path() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restore me.atomikkb");
    let content = (0..=255_u8).cycle().take(10_000).collect::<Vec<u8>>();
    fs::write(&path, &content).unwrap();

    let result = call(
        &webview,
        "backup_import",
        json!({ "path": text(&path), "kit": "legal winner thank year wave sausage worth useful" }),
    )
    .unwrap();

    assert_eq!(
        result,
        json!({
            "summary": {
                "bytes": 10_000,
                "documents": 2,
                "settings": 3,
                "createdAt": 1_700_000_000_000_i64,
                "accountId": "07070707-0707-0707-0707-070707070707",
                "chunks": 0,
                "chunkBytes": 0,
            },
            "path": text(&path),
        })
    );
    let spy = spy.lock().unwrap();
    assert_eq!(spy.imports.len(), 1, "the core is called once");
    assert!(
        spy.imports[0].bytes == content,
        "the core read different bytes than the file holds"
    );
    assert_eq!(
        spy.imports[0].kit.as_deref(),
        Some("legal winner thank year wave sausage worth useful")
    );
    // The file is only read.
    assert_eq!(fs::read(&path).unwrap(), content);
}

#[test]
fn backup_import_passes_no_kit_when_there_is_none() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");
    fs::write(&path, b"KBB1").unwrap();

    // `kit` missing, and `kit: null` (what `JSON.stringify` makes of an explicit null).
    call(&webview, "backup_import", json!({ "path": text(&path) })).unwrap();
    call(
        &webview,
        "backup_import",
        json!({ "path": text(&path), "kit": null }),
    )
    .unwrap();

    let spy = spy.lock().unwrap();
    assert_eq!(
        spy.imports,
        [
            Import {
                bytes: b"KBB1".to_vec(),
                kit: None
            },
            Import {
                bytes: b"KBB1".to_vec(),
                kit: None
            },
        ]
    );
}

#[test]
fn backup_import_needs_a_string_kit() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");
    fs::write(&path, b"KBB1").unwrap();

    let error = call(
        &webview,
        "backup_import",
        json!({ "path": text(&path), "kit": 42 }),
    )
    .unwrap_err();

    let message = error.as_str().unwrap();
    assert!(message.contains("kit"), "{message}");
    assert!(spy.lock().unwrap().imports.is_empty());
}

#[test]
fn backup_import_of_a_file_that_is_not_there_never_reaches_the_core() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.atomikkb");
    let a_directory = dir.path().to_path_buf();

    for (path, why) in [
        (text(&missing).to_owned(), "missing file"),
        (text(&a_directory).to_owned(), "a directory"),
        ("relative.atomikkb".to_owned(), "relative"),
    ] {
        let error = call(&webview, "backup_import", json!({ "path": path })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{why}");
        assert_eq!(error["details"]["field"], "path", "{why}");
    }

    assert!(spy.lock().unwrap().imports.is_empty());
}

#[test]
fn an_error_from_the_core_during_an_import_reaches_the_webview_untouched() {
    let (mut fake, spy) = Fake::new();
    fake.backup_failure = Some(ApiError::crypto(
        "the backup is damaged or the kit is wrong",
    ));
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");
    fs::write(&path, b"KBB1 garbage").unwrap();

    let error = call(
        &webview,
        "backup_import",
        json!({ "path": text(&path), "kit": "wrong kit" }),
    )
    .unwrap_err();

    assert_eq!(
        error,
        json!({ "code": "Crypto", "message": "the backup is damaged or the kit is wrong" })
    );
    assert_eq!(spy.lock().unwrap().imports.len(), 1);
    assert_eq!(
        fs::read(&path).unwrap(),
        b"KBB1 garbage",
        "an import never writes the file"
    );
}

#[test]
fn a_backup_made_by_export_can_be_imported_back() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("round trip.atomikkb");

    let exported = call(&webview, "backup_export", json!({ "path": text(&path) })).unwrap();
    let imported = call(
        &webview,
        "backup_import",
        json!({ "path": text(&path), "kit": "k" }),
    )
    .unwrap();

    assert_eq!(exported["summary"], imported["summary"]);
    assert_eq!(exported["path"], imported["path"]);
    assert_eq!(spy.lock().unwrap().imports[0].bytes, backup_bytes());
}

// --- attachments ---

/// The bytes of a raw answer.
fn raw(body: InvokeResponseBody) -> Vec<u8> {
    let InvokeResponseBody::Raw(bytes) = body else {
        unreachable!("the answer is JSON, not raw bytes");
    };
    bytes
}

/// A raw attachment call as the transport sends it: the id in its header, the bytes as the
/// body, and the range headers when there are any.
fn raw_attachment(
    webview: &WebviewWindow<MockRuntime>,
    command: &str,
    id: &str,
    body: Vec<u8>,
    range: Option<(u64, u64)>,
) -> Result<InvokeResponseBody, Value> {
    let (start, end) = range.map(|(s, e)| (s.to_string(), e.to_string())).unzip();
    let mut headers = vec![(ATTACHMENT_HEADER, id)];
    if let (Some(start), Some(end)) = (&start, &end) {
        headers.push((RANGE_START_HEADER, start.as_str()));
        headers.push((RANGE_END_HEADER, end.as_str()));
    }
    invoke(webview, command, InvokeBody::Raw(body), &headers)
}

/// `AttachmentBegin` through `dispatch`, as the client sends it; answers the id's text.
fn begin_upload(webview: &WebviewWindow<MockRuntime>, name: &str, mime: &str) -> String {
    let answer = call(
        webview,
        "dispatch",
        json!({ "request": { "type": "AttachmentBegin", "data": { "spaceId": SPACE, "name": name, "mime": mime } } }),
    )
    .unwrap();
    assert_eq!(answer["type"], "AttachmentStarted");
    answer["data"].as_str().unwrap().to_owned()
}

/// `AttachmentFinish` through `dispatch`; answers the `AttachmentInfo` JSON.
fn finish_upload(webview: &WebviewWindow<MockRuntime>, id: &str) -> Value {
    let answer = call(
        webview,
        "dispatch",
        json!({ "request": { "type": "AttachmentFinish", "data": { "attachmentId": id } } }),
    )
    .unwrap();
    assert_eq!(answer["type"], "Attachment");
    answer["data"].clone()
}

/// Puts a finished attachment straight into the fake core; answers the id's text.
fn seed_attachment(spy: &SharedSpy, name: &str, bytes: &[u8]) -> String {
    let mut spy = spy.lock().unwrap();
    spy.next_attachment += 1;
    let id = AttachmentId::from_bytes([spy.next_attachment; 16]);
    spy.stored.insert(
        id,
        Stored {
            space_id: SPACE.parse().unwrap(),
            name: name.to_owned(),
            mime: "application/octet-stream".to_owned(),
            bytes: bytes.to_vec(),
            claimed_size: None,
        },
    );
    id.to_string()
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

fn calls(spy: &SharedSpy) -> Vec<String> {
    spy.lock().unwrap().attachment_calls.clone()
}

#[test]
fn the_attachment_header_names_are_the_documented_ones() {
    // Spelled out in this test file so that a rename in the shell breaks the build of the
    // test, the TypeScript contract test and the transport together.
    assert_eq!(ATTACHMENT_HEADER, "x-kb-attachment");
    assert_eq!(RANGE_START_HEADER, "x-kb-range-start");
    assert_eq!(RANGE_END_HEADER, "x-kb-range-end");
}

#[test]
fn the_raw_attachment_commands_need_the_attachment_header() {
    let app = app();
    let webview = main_webview(&app);
    for command in ATTACHMENT_RAW_COMMANDS {
        let error = invoke(&webview, command, InvokeBody::Raw(vec![1, 2, 3]), &[]).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{command}");
        assert_eq!(error["details"]["field"], ATTACHMENT_HEADER, "{command}");
    }
}

#[test]
fn a_malformed_attachment_header_is_rejected_by_both_raw_commands() {
    let app = app();
    let webview = main_webview(&app);
    for command in ATTACHMENT_RAW_COMMANDS {
        for bad in ["not-a-uuid", "", "../../etc/passwd", PAGE_HEADER] {
            let error = raw_attachment(&webview, command, bad, vec![1], None).unwrap_err();
            assert_eq!(error["code"], "InvalidInput", "{command} {bad:?}");
            assert_eq!(
                error["details"]["field"], ATTACHMENT_HEADER,
                "{command} {bad:?}"
            );
        }
    }
}

#[test]
fn a_valid_raw_attachment_call_reaches_the_core() {
    let app = app();
    let webview = main_webview(&app);
    for command in ATTACHMENT_RAW_COMMANDS {
        let error = raw_attachment(&webview, command, ATTACHMENT, vec![1, 2, 3], None).unwrap_err();
        assert_eq!(error, starting(), "{command}");
    }
    // A range does not change that.
    let error = raw_attachment(
        &webview,
        "attachment_read",
        ATTACHMENT,
        Vec::new(),
        Some((0, 10)),
    )
    .unwrap_err();
    assert_eq!(error, starting());
}

#[test]
fn every_body_shape_the_transport_can_send_reaches_attachment_write_and_read() {
    let app = app();
    let webview = main_webview(&app);
    let headers = [(ATTACHMENT_HEADER, ATTACHMENT)];
    // attachment_read ignores its body; attachment_write takes bytes.
    for body in [
        InvokeBody::Raw(vec![0, 255]),
        // what the postMessage fallback makes of a Uint8Array
        InvokeBody::Json(json!([0, 255])),
    ] {
        for command in ATTACHMENT_RAW_COMMANDS {
            let error = invoke(&webview, command, body.clone(), &headers).unwrap_err();
            assert_eq!(error, starting(), "{command} with {body:?}");
        }
    }
    for body in [
        InvokeBody::Raw(Vec::new()),
        InvokeBody::Json(json!({})),
        InvokeBody::Json(Value::Null),
        InvokeBody::default(),
    ] {
        let error = invoke(&webview, "attachment_read", body.clone(), &headers).unwrap_err();
        assert_eq!(error, starting(), "attachment_read with {body:?}");
    }
}

#[test]
fn attachment_write_refuses_an_empty_or_oversized_piece_before_it_asks_the_core() {
    let app = app();
    let webview = main_webview(&app);
    for body in [
        InvokeBody::Raw(Vec::new()),
        InvokeBody::Json(json!({})),
        InvokeBody::Json(Value::Null),
        InvokeBody::Json(json!([])),
    ] {
        let error = invoke(
            &webview,
            "attachment_write",
            body.clone(),
            &[(ATTACHMENT_HEADER, ATTACHMENT)],
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{body:?}");
        assert_eq!(error["details"]["field"], "body", "{body:?}");
    }

    let error = raw_attachment(
        &webview,
        "attachment_write",
        ATTACHMENT,
        vec![0; CHUNK_SIZE + 1],
        None,
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "body");
    assert_eq!(error["details"]["limit"], CHUNK_SIZE);

    // One chunk is the most there is room for, and it reaches the core.
    let error = raw_attachment(
        &webview,
        "attachment_write",
        ATTACHMENT,
        vec![0; CHUNK_SIZE],
        None,
    )
    .unwrap_err();
    assert_eq!(error, starting());
}

#[test]
fn attachment_read_refuses_bad_range_headers_before_it_asks_the_core() {
    let app = app();
    let webview = main_webview(&app);
    let read = |headers: &[(&str, &str)]| {
        let mut all = vec![(ATTACHMENT_HEADER, ATTACHMENT)];
        all.extend_from_slice(headers);
        invoke(
            &webview,
            "attachment_read",
            InvokeBody::Raw(Vec::new()),
            &all,
        )
        .unwrap_err()
    };
    let too_long = (MAX_PREVIEW_BYTES + 1).to_string();

    for (headers, field) in [
        (vec![(RANGE_START_HEADER, "0")], RANGE_END_HEADER),
        (vec![(RANGE_END_HEADER, "9")], RANGE_START_HEADER),
        (
            vec![(RANGE_START_HEADER, "x"), (RANGE_END_HEADER, "9")],
            RANGE_START_HEADER,
        ),
        (
            vec![(RANGE_START_HEADER, "0"), (RANGE_END_HEADER, "-9")],
            RANGE_END_HEADER,
        ),
        (
            vec![(RANGE_START_HEADER, "5"), (RANGE_END_HEADER, "5")],
            "range",
        ),
        (
            vec![(RANGE_START_HEADER, "9"), (RANGE_END_HEADER, "5")],
            "range",
        ),
        (
            vec![
                (RANGE_START_HEADER, "0"),
                (RANGE_END_HEADER, too_long.as_str()),
            ],
            "range",
        ),
    ] {
        let error = read(&headers);
        assert_eq!(error["code"], "InvalidInput", "{headers:?}");
        assert_eq!(error["details"]["field"], field, "{headers:?}");
    }
}

#[test]
fn attachments_round_trip_through_the_raw_commands() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let content = pattern(25);

    let id = begin_upload(&webview, "photo.png", "image/png");
    for piece in content.chunks(10) {
        let answer =
            raw_attachment(&webview, "attachment_write", &id, piece.to_vec(), None).unwrap();
        assert!(raw(answer).is_empty(), "attachment_write answers empty");
    }
    let info = finish_upload(&webview, &id);
    assert_eq!(info["attachmentId"], id.as_str());
    assert_eq!(info["size"], 25);
    assert_eq!(info["name"], "photo.png");
    assert_eq!(info["mime"], "image/png");

    let all = raw(raw_attachment(&webview, "attachment_read", &id, Vec::new(), None).unwrap());
    assert_eq!(all, content);
    let part =
        raw(raw_attachment(&webview, "attachment_read", &id, Vec::new(), Some((3, 17))).unwrap());
    assert_eq!(part, &content[3..17]);
    let tail =
        raw(raw_attachment(&webview, "attachment_read", &id, Vec::new(), Some((24, 25))).unwrap());
    assert_eq!(tail, &content[24..]);

    assert_eq!(
        calls(&spy),
        [
            "begin",
            "write:10",
            "write:10",
            "write:5",
            "finish",
            "get",
            "read:full",
            "read:3-17",
            "read:24-25",
        ]
    );
}

#[test]
fn a_whole_read_above_the_preview_limit_is_refused_but_a_range_is_not() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let id = seed_attachment(&spy, "big.bin", b"a stand-in for a large file");
    let big: AttachmentId = id.parse().unwrap();
    spy.lock()
        .unwrap()
        .stored
        .get_mut(&big)
        .unwrap()
        .claimed_size = Some(MAX_PREVIEW_BYTES + 1);

    let error = raw_attachment(&webview, "attachment_read", &id, Vec::new(), None).unwrap_err();

    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "size");
    assert_eq!(error["details"]["limit"], MAX_PREVIEW_BYTES);
    assert_eq!(calls(&spy), ["get"], "nothing may be read");

    let part =
        raw(raw_attachment(&webview, "attachment_read", &id, Vec::new(), Some((2, 9))).unwrap());
    assert_eq!(part, b"stand-i");
}

#[test]
fn an_attachment_of_exactly_the_preview_limit_is_not_refused_by_size() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let id = seed_attachment(&spy, "big.bin", b"stand-in");
    let big: AttachmentId = id.parse().unwrap();
    spy.lock()
        .unwrap()
        .stored
        .get_mut(&big)
        .unwrap()
        .claimed_size = Some(MAX_PREVIEW_BYTES);

    // The fake holds fewer bytes than it claims, which the shell notices: that is an
    // internal error, not the size refusal.
    let error = raw_attachment(&webview, "attachment_read", &id, Vec::new(), None).unwrap_err();
    assert_eq!(error["code"], "Internal");
}

#[test]
fn errors_of_the_core_reach_the_webview_from_a_raw_read() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    let unknown = AttachmentId::from_bytes([200; 16]).to_string();
    for range in [None, Some((0, 5))] {
        let error =
            raw_attachment(&webview, "attachment_read", &unknown, Vec::new(), range).unwrap_err();
        assert_eq!(error["code"], "NotFound", "{range:?}");
    }

    let id = seed_attachment(&spy, "a.bin", &pattern(10));
    let error =
        raw_attachment(&webview, "attachment_read", &id, Vec::new(), Some((5, 11))).unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "range");

    spy.lock().unwrap().fail_read = Some(ApiError::crypto("a chunk does not open"));
    let error = raw_attachment(&webview, "attachment_read", &id, Vec::new(), None).unwrap_err();
    assert_eq!(
        error,
        json!({ "code": "Crypto", "message": "a chunk does not open" })
    );
}

#[test]
fn a_write_to_an_unknown_upload_is_the_core_s_not_found() {
    let (fake, _spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let error =
        raw_attachment(&webview, "attachment_write", ATTACHMENT, vec![1], None).unwrap_err();
    assert_eq!(error["code"], "NotFound");
}

#[test]
fn attachment_import_path_streams_a_file_into_a_new_attachment() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let content = pattern(2 * CHUNK_SIZE + 3);
    let path = dir.path().join("Holiday Photo.PNG");
    fs::write(&path, &content).unwrap();

    let info = call(
        &webview,
        "attachment_import_path",
        json!({ "spaceId": SPACE, "path": text(&path) }),
    )
    .unwrap();

    assert_eq!(info["name"], "Holiday Photo.PNG");
    assert_eq!(info["mime"], "image/png");
    assert_eq!(info["size"], content.len());
    assert_eq!(info["spaceId"], SPACE);
    assert_eq!(info["chunkCount"], 3);
    assert_eq!(info["references"], 0);
    let id: AttachmentId = info["attachmentId"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        calls(&spy),
        [
            "begin".to_owned(),
            format!("write:{CHUNK_SIZE}"),
            format!("write:{CHUNK_SIZE}"),
            "write:3".to_owned(),
            "finish".to_owned(),
        ]
    );
    let spy = spy.lock().unwrap();
    assert!(
        spy.stored[&id].bytes == content,
        "the core received other bytes than the file holds"
    );
    // The file is only read.
    assert_eq!(fs::read(&path).unwrap(), content);
}

#[test]
fn attachment_import_path_takes_the_mime_type_from_the_extension() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    for (name, mime) in [
        ("a.pdf", "application/pdf"),
        ("b.JPG", "image/jpeg"),
        ("c.mp4", "video/mp4"),
        ("d.zzz", "application/octet-stream"),
        ("e", "application/octet-stream"),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, b"x").unwrap();
        let info = call(
            &webview,
            "attachment_import_path",
            json!({ "spaceId": SPACE, "path": text(&path) }),
        )
        .unwrap();
        assert_eq!(info["mime"], mime, "{name}");
    }
    assert_eq!(spy.lock().unwrap().stored.len(), 5);
}

#[test]
fn an_empty_file_is_imported_without_a_write() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.txt");
    fs::write(&path, b"").unwrap();

    let info = call(
        &webview,
        "attachment_import_path",
        json!({ "spaceId": SPACE, "path": text(&path) }),
    )
    .unwrap();

    assert_eq!(info["size"], 0);
    assert_eq!(calls(&spy), ["begin", "finish"]);
}

#[test]
fn a_failed_import_aborts_the_upload_and_answers_the_error() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.bin");
    fs::write(&path, pattern(CHUNK_SIZE + 1)).unwrap();
    spy.lock().unwrap().fail_write_on = Some(2);

    let error = call(
        &webview,
        "attachment_import_path",
        json!({ "spaceId": SPACE, "path": text(&path) }),
    )
    .unwrap_err();

    assert_eq!(error["code"], "Storage");
    assert_eq!(
        calls(&spy),
        [
            "begin".to_owned(),
            format!("write:{CHUNK_SIZE}"),
            "write:1".to_owned(),
            "abort".to_owned(),
        ]
    );
    let spy = spy.lock().unwrap();
    assert!(spy.uploads.is_empty(), "the upload was left behind");
    assert!(spy.stored.is_empty());
}

#[test]
fn attachment_import_path_refuses_a_bad_path_before_the_core_runs() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.bin");
    let a_directory = dir.path().to_path_buf();

    for (path, why) in [
        ("photo.png".to_owned(), "relative"),
        (String::new(), "empty"),
        (text(&missing).to_owned(), "missing file"),
        (text(&a_directory).to_owned(), "a directory"),
    ] {
        let error = call(
            &webview,
            "attachment_import_path",
            json!({ "spaceId": SPACE, "path": path }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{why}");
        assert_eq!(error["details"]["field"], "path", "{why}");
    }
    assert!(calls(&spy).is_empty());
}

#[test]
fn attachment_import_path_needs_a_space_id_and_a_string_path() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    for (args, mentions) in [
        (json!({}), "spaceId"),
        (json!({ "spaceId": 5 }), "spaceId"),
        (json!({ "spaceId": "not-an-id" }), "spaceId"),
        (json!({ "spaceId": SPACE, "path": 5 }), "path"),
    ] {
        let error = call(&webview, "attachment_import_path", args.clone()).unwrap_err();
        let message = error.as_str().unwrap();
        assert!(message.contains(mentions), "{args}: {message}");
    }
    assert!(calls(&spy).is_empty());
}

#[test]
fn attachment_import_url_refuses_a_bad_link_before_any_request_or_core_call() {
    // Every one of these is refused by the link's own rules, so the test needs no network: a
    // link that got past them would be a request to a real server.
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let too_long = format!("https://example.com/{}", "a".repeat(2100));

    for link in [
        "http://example.com/a.png",
        "file:///C:/Users/me/a.png",
        "data:image/png;base64,iVBORw0KGgo=",
        "javascript:alert(1)",
        "https://user:secret@example.com/a.png",
        "",
        "   ",
        "example.com/a.png",
        "not a link",
        too_long.as_str(),
    ] {
        let error = call(
            &webview,
            "attachment_import_url",
            json!({ "spaceId": SPACE, "url": link }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{link:?}");
        assert_eq!(error["details"]["field"], "url", "{link:?}");
        let message = error["message"].as_str().unwrap();
        assert!(
            !message.contains("example.com") && !message.contains("secret"),
            "{link:?}: the message repeats the link: {message}"
        );
    }
    assert!(calls(&spy).is_empty());
}

#[test]
fn attachment_import_url_needs_a_space_id_and_a_string_url() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    for (args, mentions) in [
        (json!({}), "spaceId"),
        (json!({ "url": "https://example.com/a.png" }), "spaceId"),
        (json!({ "spaceId": SPACE }), "url"),
        (
            json!({ "spaceId": 5, "url": "https://example.com/a.png" }),
            "spaceId",
        ),
        (
            json!({ "spaceId": "not-an-id", "url": "https://example.com/a.png" }),
            "spaceId",
        ),
        (json!({ "spaceId": SPACE, "url": 5 }), "url"),
        (json!({ "spaceId": SPACE, "url": null }), "url"),
    ] {
        let error = call(&webview, "attachment_import_url", args.clone()).unwrap_err();
        let message = error.as_str().unwrap();
        assert!(message.contains(mentions), "{args}: {message}");
    }
    assert!(calls(&spy).is_empty());
}

#[test]
fn attachment_save_as_writes_the_plaintext_to_the_file_and_answers_info_and_path() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let content = pattern(1000);
    let id = seed_attachment(&spy, "photo.png", &content);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("my copy.png");

    let result = call(
        &webview,
        "attachment_save_as",
        json!({ "attachmentId": id, "path": text(&path) }),
    )
    .unwrap();

    assert_eq!(result["path"], text(&path));
    assert_eq!(result["info"]["attachmentId"], id.as_str());
    assert_eq!(result["info"]["name"], "photo.png");
    assert_eq!(result["info"]["size"], 1000);
    assert_eq!(
        result.as_object().unwrap().len(),
        2,
        "the answer is exactly {{ info, path }}: {result}"
    );
    assert_eq!(fs::read(&path).unwrap(), content);
    // The temporary file is gone.
    assert_eq!(files_in(dir.path()), ["my copy.png"]);
    assert_eq!(calls(&spy), ["get", "read:full"]);
}

#[test]
fn attachment_save_as_replaces_a_file_that_is_already_there() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let id = seed_attachment(&spy, "a.txt", b"new");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, b"the previous content, which is longer").unwrap();

    call(
        &webview,
        "attachment_save_as",
        json!({ "attachmentId": id, "path": text(&path) }),
    )
    .unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(files_in(dir.path()), ["a.txt"]);
}

#[test]
fn a_failed_save_leaves_neither_a_new_file_nor_a_temporary_one_and_keeps_the_old_file() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let id = seed_attachment(&spy, "a.txt", &pattern(100));
    spy.lock().unwrap().fail_read = Some(ApiError::crypto("a chunk does not open"));
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh.txt");
    let existing = dir.path().join("existing.txt");
    fs::write(&existing, b"precious").unwrap();

    for path in [&fresh, &existing] {
        let error = call(
            &webview,
            "attachment_save_as",
            json!({ "attachmentId": id, "path": text(path) }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "Crypto");
    }

    assert_eq!(files_in(dir.path()), ["existing.txt"]);
    assert_eq!(fs::read(&existing).unwrap(), b"precious");
}

#[test]
fn attachment_save_as_refuses_a_bad_path_and_an_unknown_attachment() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let id = seed_attachment(&spy, "a.txt", b"x");
    let dir = tempfile::tempdir().unwrap();
    let missing_directory = dir.path().join("no-such-dir").join("a.txt");

    for (path, why) in [
        ("a.txt".to_owned(), "relative"),
        (String::new(), "empty"),
        (text(&missing_directory).to_owned(), "missing directory"),
        (text(dir.path()).to_owned(), "a directory"),
    ] {
        let error = call(
            &webview,
            "attachment_save_as",
            json!({ "attachmentId": id, "path": path }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{why}");
        assert_eq!(error["details"]["field"], "path", "{why}");
    }

    let unknown = AttachmentId::from_bytes([77; 16]).to_string();
    let path = dir.path().join("x.txt");
    let error = call(
        &webview,
        "attachment_save_as",
        json!({ "attachmentId": unknown, "path": text(&path) }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "NotFound");
    assert!(files_in(dir.path()).is_empty());
    // Nothing was read for any of them.
    assert!(!calls(&spy).iter().any(|call| call.starts_with("read")));
}

/// The `path` argument is for the dev app, the smoke scripts and these tests: a release
/// build refuses it and leaves the choice to the native dialog. The expectation follows the
/// build; every other test of the attachment commands here uses explicit paths and so
/// assumes a debug build (the default for `cargo test`, which `scripts/check.ps1` runs).
#[test]
fn a_given_path_is_honoured_in_debug_builds_and_refused_in_release_builds() {
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    fs::write(&source, b"plaintext").unwrap();
    let id = seed_attachment(&spy, "a.txt", b"saved");
    let saved = dir.path().join("saved.txt");

    let imported = call(
        &webview,
        "attachment_import_path",
        json!({ "spaceId": SPACE, "path": text(&source) }),
    );
    let saved_as = call(
        &webview,
        "attachment_save_as",
        json!({ "attachmentId": id, "path": text(&saved) }),
    );

    if cfg!(debug_assertions) {
        assert_eq!(imported.unwrap()["name"], "source.txt");
        assert_eq!(saved_as.unwrap()["path"], text(&saved));
        assert_eq!(fs::read(&saved).unwrap(), b"saved");
    } else {
        let expected = json!({
            "code": "InvalidInput",
            "message": "choose the file in the dialog",
            "details": { "field": "path" },
        });
        assert_eq!(imported.unwrap_err(), expected);
        assert_eq!(saved_as.unwrap_err(), expected);
        // Nothing was asked of the core and no file was made.
        assert!(calls(&spy).is_empty());
        assert_eq!(files_in(dir.path()), ["source.txt"]);
    }
}

#[test]
fn attachment_save_as_and_open_need_an_attachment_id() {
    let (fake, _spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    for command in ["attachment_save_as", "attachment_open"] {
        for args in [
            json!({}),
            json!({ "attachmentId": 5 }),
            json!({ "attachmentId": "not-an-id" }),
        ] {
            let error = call(&webview, command, args.clone()).unwrap_err();
            let message = error.as_str().unwrap();
            assert!(
                message.contains("attachmentId"),
                "{command} {args}: {message}"
            );
        }
    }
}

#[test]
fn attachment_open_refuses_files_the_system_would_run_and_unknown_attachments() {
    // Only the refusals: a file that passes would be handed to the system opener.
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);

    for name in [
        "setup.exe",
        "RUN.BAT",
        "link.lnk",
        "script.ps1",
        "deploy.sh",
    ] {
        let id = seed_attachment(&spy, name, b"MZ");
        let error = call(&webview, "attachment_open", json!({ "attachmentId": id })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{name}");
        assert_eq!(error["details"]["field"], "attachment", "{name}");
    }

    let unknown = AttachmentId::from_bytes([88; 16]).to_string();
    let error = call(
        &webview,
        "attachment_open",
        json!({ "attachmentId": unknown }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "NotFound");
    // Nothing was decrypted.
    assert!(!calls(&spy).iter().any(|call| call.starts_with("read")));
}

// --- secrets stay out of the log ---

/// Everything logged anywhere in this test binary since [`install_log_capture`].
static LOG: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

#[derive(Clone)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Installs a process-wide subscriber (the commands log from Tauri's worker threads, so a
/// thread-local one would see nothing) and returns the buffer it writes to.
fn install_log_capture() -> Arc<Mutex<Vec<u8>>> {
    Arc::clone(LOG.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(Arc::clone(&buffer));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        // A second subscriber cannot be installed; then the assertions below on what was
        // logged fail loudly instead of passing on an empty log.
        let _ = tracing::subscriber::set_global_default(subscriber);
        buffer
    }))
}

#[test]
fn neither_the_passphrase_nor_the_kit_is_logged_by_any_command() {
    let log = install_log_capture();
    let (fake, _spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.atomikkb");
    fs::write(&path, b"KBB1").unwrap();

    call(&webview, "lock", json!({})).unwrap();
    let _ = call(
        &webview,
        "unlock",
        json!({ "passphrase": "wrong-SENTINEL-4471" }),
    )
    .unwrap_err();
    call(&webview, "unlock", json!({ "passphrase": "open sesame" })).unwrap();
    call(
        &webview,
        "backup_import",
        json!({ "path": text(&path), "kit": "kit-SENTINEL-9913" }),
    )
    .unwrap();

    let logs = String::from_utf8_lossy(&log.lock().unwrap()).into_owned();
    assert!(
        logs.contains("core state changed"),
        "the log capture saw nothing: {logs}"
    );
    assert!(logs.contains("backup imported"), "{logs}");
    assert!(
        !logs.contains("SENTINEL"),
        "a secret reached the log: {logs}"
    );
    assert!(
        !logs.contains("open sesame"),
        "the passphrase reached the log: {logs}"
    );
}

#[test]
fn no_attachment_command_logs_a_name_a_type_a_path_or_content() {
    let log = install_log_capture();
    let (fake, spy) = Fake::new();
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ATTACHSECRET-name.pdf");
    fs::write(&source, b"ATTACHSECRET-content").unwrap();

    // Import, raw read, save as, a refused open and a failed import.
    let info = call(
        &webview,
        "attachment_import_path",
        json!({ "spaceId": SPACE, "path": text(&source) }),
    )
    .unwrap();
    let id = info["attachmentId"].as_str().unwrap().to_owned();
    raw(raw_attachment(&webview, "attachment_read", &id, Vec::new(), None).unwrap());
    let saved = dir.path().join("ATTACHSECRET-saved.pdf");
    call(
        &webview,
        "attachment_save_as",
        json!({ "attachmentId": id, "path": text(&saved) }),
    )
    .unwrap();
    let exe = seed_attachment(&spy, "ATTACHSECRET-run.exe", b"MZ");
    call(&webview, "attachment_open", json!({ "attachmentId": exe })).unwrap_err();
    let writes_so_far = spy.lock().unwrap().writes;
    spy.lock().unwrap().fail_write_on = Some(writes_so_far + 1);
    call(
        &webview,
        "attachment_import_path",
        json!({ "spaceId": SPACE, "path": text(&source) }),
    )
    .unwrap_err();
    // A refused link (no request is made): neither it nor its host is logged.
    call(
        &webview,
        "attachment_import_url",
        json!({ "spaceId": SPACE, "url": "http://ATTACHSECRET.invalid/ATTACHSECRET.png?t=ATTACHSECRET" }),
    )
    .unwrap_err();

    let logs = String::from_utf8_lossy(&log.lock().unwrap()).into_owned();
    assert!(
        logs.contains("attachment imported"),
        "the log capture saw nothing: {logs}"
    );
    assert!(logs.contains("image import from a link failed"), "{logs}");
    assert!(logs.contains("attachment saved"), "{logs}");
    assert!(logs.contains("attachment import failed"), "{logs}");
    assert!(logs.contains("attachment_read"), "{logs}");
    for secret in [
        "ATTACHSECRET",
        "application/pdf",
        "application/octet-stream",
    ] {
        assert!(!logs.contains(secret), "{secret} reached the log: {logs}");
    }
}

// --- the main window and its permissions ---

#[test]
fn the_main_window_is_declared_for_the_shell_to_build() {
    let app = app();
    let window = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .expect("tauri.conf.json declares the main window");
    assert!(
        !window.create,
        "the window must say \"create\": false, or Tauri builds it without the navigation guard"
    );
    assert_eq!(window.title, "atomik-kb");
    assert_eq!((window.width, window.height), (1100.0, 760.0));
    assert_eq!(
        (window.min_width, window.min_height),
        (Some(800.0), Some(560.0))
    );
    assert!(!window.drag_drop_enabled);
    assert_eq!(app.config().app.windows.len(), 1);
}

#[test]
fn the_shell_builds_the_main_window_from_its_config() {
    let app = app();
    assert!(app.get_webview_window(MAIN_WINDOW).is_none());
    create_main_window(&app).unwrap();
    assert!(app.get_webview_window(MAIN_WINDOW).is_some());
}

#[test]
fn building_the_main_window_fails_loudly_without_a_config_entry() {
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let error = create_main_window(&app).unwrap_err().to_string();
    assert!(error.contains("main"), "{error}");
}

#[test]
fn the_capability_grants_the_core_permissions_and_nothing_else() {
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    assert_eq!(capability["windows"], json!([MAIN_WINDOW]));
    assert_eq!(capability["permissions"], json!(["core:default"]));
    assert!(
        capability.get("remote").is_none(),
        "no remote origin may use IPC"
    );
}

#[test]
fn the_capability_describes_every_command_the_shell_registers() {
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    let description = capability["description"].as_str().unwrap();
    for command in [
        "dispatch",
        "subscribe_events",
        "core_state",
        "doc_open",
        "doc_import",
        "doc_export",
        "doc_close",
        "open_external",
        "lock",
        "unlock",
        "backup_export",
        "backup_import",
        "attachment_write",
        "attachment_read",
        "attachment_import_path",
        "attachment_import_url",
        "attachment_save_as",
        "attachment_open",
        "export_run",
        "import_run",
        "save_text_file",
        "backup_folder_choose",
        "backup_run_now",
        "shell_events_subscribe",
        "shell_labels_set",
        "menu_bar_show",
        "menu_bar_hide",
        "presence_send",
        "presence_events",
        "dictation_status",
        "dictation_select",
        "dictation_install",
        "dictation_install_cancel",
        "dictation_import",
        "dictation_remove",
        "dictation_start",
        "dictation_audio",
        "dictation_stop",
        "dictation_cancel",
        "drive_status",
        "drive_connect",
        "drive_disconnect",
        "drive_client_set",
        "drive_backups_list",
        "drive_backup_download",
        "drive_cancel",
        "backup_folder_list",
    ] {
        assert!(
            description.contains(command),
            "{command} is not described: {description}"
        );
    }
}

#[test]
fn the_webview_cannot_call_the_opener_plugin_directly() {
    let app = app();
    let webview = main_webview(&app);
    // Arguments that could do no harm even if the call were allowed (no default scope of
    // the plugin covers an ftp link, and nothing exists at these paths).
    for (command, args) in [
        (
            "plugin:opener|open_url",
            json!({ "url": "ftp://nothing.invalid/" }),
        ),
        (
            "plugin:opener|open_path",
            json!({ "path": "nothing-here.invalid" }),
        ),
        (
            "plugin:opener|reveal_item_in_dir",
            json!({ "paths": ["nothing-here.invalid"] }),
        ),
    ] {
        let error = invoke(&webview, command, InvokeBody::Json(args), &[]).unwrap_err();
        let message = error.as_str().unwrap_or_default();
        assert!(message.contains("not allowed"), "{command}: {error}");
    }
}

#[test]
fn the_webview_cannot_call_the_notification_plugin_directly() {
    let app = app();
    let webview = main_webview(&app);
    // The shell shows the OS notification of a due reminder from Rust, which needs no
    // permission; the webview has none, so a page cannot show one (the plugin replaces
    // `window.Notification` in the page, and this is where that call lands). The arguments
    // are nonsense on purpose: if the call were ever allowed it would fail to deserialise
    // instead of showing a notification.
    for command in [
        "plugin:notification|notify",
        "plugin:notification|request_permission",
        "plugin:notification|is_permission_granted",
    ] {
        let error = invoke(
            &webview,
            command,
            InvokeBody::Json(json!({ "options": "not an options object" })),
            &[],
        )
        .unwrap_err();
        let message = error.as_str().unwrap_or_default();
        assert!(message.contains("not allowed"), "{command}: {error}");
    }
}

#[test]
fn the_webview_cannot_call_the_dialog_plugin_directly() {
    let app = app();
    let webview = main_webview(&app);
    // The shell opens its dialogs from Rust, which needs no permission; the webview has
    // none, so it cannot open one itself. The arguments are nonsense on purpose: if the
    // call were ever allowed it would fail to deserialise instead of opening a dialog
    // that nobody can answer.
    for command in [
        "plugin:dialog|open",
        "plugin:dialog|save",
        "plugin:dialog|message",
    ] {
        let error = invoke(
            &webview,
            command,
            InvokeBody::Json(json!({ "options": "not an options object" })),
            &[],
        )
        .unwrap_err();
        let message = error.as_str().unwrap_or_default();
        assert!(message.contains("not allowed"), "{command}: {error}");
    }
}

// --- presence in shared spaces (M13 slice 3, ADR-0050): presence_send and presence_events ---

fn presence_feed() -> FeedId {
    FeedId::from_bytes([6; 16])
}

/// The seal the fake core answers: a frame for the space's feed.
fn sealed_frame() -> SealedPresence {
    SealedPresence {
        feed: presence_feed(),
        envelope: vec![9; 64],
    }
}

/// An app whose core seals into [`sealed_frame`], and a socket whose queue the test reads.
fn presence_app() -> (
    App<MockRuntime>,
    Arc<AppState<Fake>>,
    SharedSpy,
    tokio::sync::mpsc::Receiver<RealtimeCommand>,
) {
    let (fake, spy) = Fake::new();
    spy.lock().unwrap().sealed = Some(sealed_frame());
    let (app, state) = fake_app(fake);
    let (commands, socket) = tokio::sync::mpsc::channel(64);
    state.presence_attach(RealtimeHandle::connected_to(commands));
    (app, state, spy, socket)
}

#[test]
fn presence_send_takes_the_arguments_the_transport_sends_seals_in_the_core_and_hands_the_frame_to_the_socket()
 {
    let (app, _state, spy, mut socket) = presence_app();
    let webview = main_webview(&app);
    let answer = call(
        &webview,
        "presence_send",
        json!({ "spaceId": SPACE, "pageId": PAGE, "state": "Editing", "update": [1, 2, 3] }),
    )
    .unwrap();
    assert_eq!(answer, Value::Null, "it answers nothing");
    let seals = spy.lock().unwrap().seals.clone();
    assert_eq!(seals.len(), 1);
    assert_eq!(serde_json::to_value(seals[0].0).unwrap(), json!(SPACE));
    assert_eq!(serde_json::to_value(seals[0].1).unwrap(), json!(PAGE));
    assert_eq!(seals[0].2, PresenceState::Editing);
    assert_eq!(
        seals[0].3,
        vec![1, 2, 3],
        "the bytes, as the array of values it was sent as"
    );
    assert_eq!(
        socket.try_recv().unwrap(),
        RealtimeCommand::Presence(PresenceFrame {
            feed: presence_feed(),
            envelope: vec![9; 64],
        })
    );

    // `pageId` may be left out, or null, for "in no page"; the other two states and an empty update.
    for args in [
        json!({ "spaceId": SPACE, "state": "Idle", "update": [] }),
        json!({ "spaceId": SPACE, "pageId": null, "state": "Viewing", "update": [255, 0] }),
    ] {
        call(&webview, "presence_send", args).unwrap();
    }
    let seals = spy.lock().unwrap().seals.clone();
    assert_eq!(seals[1].1, None);
    assert_eq!(seals[1].2, PresenceState::Idle);
    assert_eq!(seals[2].1, None);
    assert_eq!(seals[2].2, PresenceState::Viewing);
    assert_eq!(seals[2].3, vec![255, 0]);
}

#[test]
fn presence_send_refuses_arguments_that_are_not_the_documented_ones_before_the_core_runs() {
    let (app, _state, spy, mut socket) = presence_app();
    let webview = main_webview(&app);
    for (args, field) in [
        // A state that is not one of the three.
        (
            json!({ "spaceId": SPACE, "state": "Dancing", "update": [] }),
            "state",
        ),
        // An update that is not an array of bytes: text, a number outside a byte, a nested array.
        (
            json!({ "spaceId": SPACE, "state": "Idle", "update": "bytes" }),
            "update",
        ),
        (
            json!({ "spaceId": SPACE, "state": "Idle", "update": [256] }),
            "update",
        ),
        (
            json!({ "spaceId": SPACE, "state": "Idle", "update": [[1]] }),
            "update",
        ),
        // Missing arguments, and ids that are not ids.
        (json!({ "state": "Idle", "update": [] }), "space"),
        (json!({ "spaceId": SPACE, "update": [] }), "state"),
        (json!({ "spaceId": SPACE, "state": "Idle" }), "update"),
        (
            json!({ "spaceId": "not an id", "state": "Idle", "update": [] }),
            "space",
        ),
        (
            json!({ "spaceId": SPACE, "pageId": 7, "state": "Idle", "update": [] }),
            "page",
        ),
    ] {
        let error = call(&webview, "presence_send", args.clone()).unwrap_err();
        let text = error.to_string().to_lowercase();
        assert!(
            text.contains(field),
            "{args}: the refusal does not name `{field}`: {error}"
        );
    }
    assert!(spy.lock().unwrap().seals.is_empty(), "the core never ran");
    assert!(socket.try_recv().is_err());
}

#[test]
fn an_update_over_three_kib_is_invalid_input_for_update_and_the_largest_one_is_fine() {
    let (app, _state, spy, _socket) = presence_app();
    let webview = main_webview(&app);
    let too_long: Vec<u8> = vec![7; kb_types::sharing::PRESENCE_UPDATE_MAX_BYTES + 1];
    let error = call(
        &webview,
        "presence_send",
        json!({ "spaceId": SPACE, "state": "Editing", "update": too_long }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "update");
    assert!(spy.lock().unwrap().seals.is_empty());
    let largest: Vec<u8> = vec![7; kb_types::sharing::PRESENCE_UPDATE_MAX_BYTES];
    call(
        &webview,
        "presence_send",
        json!({ "spaceId": SPACE, "state": "Editing", "update": largest }),
    )
    .unwrap();
    assert_eq!(spy.lock().unwrap().seals.len(), 1);
}

#[test]
fn a_space_with_nothing_to_send_and_a_device_with_no_socket_answer_ok() {
    // The core seals nothing (a personal space, an archive): `Ok`, and nothing reaches the socket.
    let (app, _state, spy, mut socket) = presence_app();
    spy.lock().unwrap().sealed = None;
    let webview = main_webview(&app);
    let args = json!({ "spaceId": SPACE, "pageId": PAGE, "state": "Viewing", "update": [1] });
    assert_eq!(
        call(&webview, "presence_send", args.clone()).unwrap(),
        Value::Null
    );
    assert!(socket.try_recv().is_err());
    assert_eq!(spy.lock().unwrap().seals.len(), 1);
    // Sealed, but no driver is running (sync is off, or it stopped): the frame has nowhere to go.
    let (fake, spy) = Fake::new();
    spy.lock().unwrap().sealed = Some(sealed_frame());
    let (app, _state) = fake_app(fake);
    let webview = main_webview(&app);
    assert_eq!(call(&webview, "presence_send", args).unwrap(), Value::Null);
    assert_eq!(spy.lock().unwrap().seals.len(), 1);
}

#[test]
fn at_most_ten_presence_updates_a_second_reach_the_core_and_the_rest_answer_ok() {
    let (app, _state, spy, mut socket) = presence_app();
    let webview = main_webview(&app);
    for i in 0..30_u8 {
        call(
            &webview,
            "presence_send",
            json!({ "spaceId": SPACE, "state": "Editing", "update": [i] }),
        )
        .unwrap();
    }
    let seals = spy.lock().unwrap().seals.clone();
    assert_eq!(seals.len(), 10, "the window of one second holds ten");
    assert_eq!(
        seals.iter().map(|seal| seal.3[0]).collect::<Vec<_>>(),
        (0..10).collect::<Vec<_>>(),
        "the first ten, in order"
    );
    let mut sent = 0;
    while socket.try_recv().is_ok() {
        sent += 1;
    }
    assert_eq!(sent, 10);
}

#[test]
fn presence_send_is_answered_like_any_command_while_the_core_is_starting_or_locked() {
    let args = json!({ "spaceId": SPACE, "state": "Idle", "update": [] });
    let app = app();
    let webview = main_webview(&app);
    assert_eq!(
        call(&webview, "presence_send", args.clone()).unwrap_err(),
        starting()
    );

    let (app, _state, spy, mut socket) = presence_app();
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();
    assert_eq!(call(&webview, "presence_send", args).unwrap_err(), locked());
    assert!(spy.lock().unwrap().seals.is_empty());
    assert!(socket.try_recv().is_err());
}

#[test]
fn a_refusal_of_the_core_reaches_the_webview_untouched() {
    // An unknown space, or a page that is not in it: the core's own `InvalidInput`.
    let (app, _state, spy, mut socket) = presence_app();
    spy.lock().unwrap().seal_error =
        Some(ApiError::invalid("pageId", "the page is not in this space"));
    let webview = main_webview(&app);
    let error = call(
        &webview,
        "presence_send",
        json!({ "spaceId": SPACE, "pageId": PAGE, "state": "Idle", "update": [] }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "pageId");
    assert_eq!(error["message"], "the page is not in this space");
    assert!(socket.try_recv().is_err());
}

#[test]
fn presence_events_registers_the_channel_the_way_the_transport_sends_it() {
    let (app, state, _spy, _socket) = presence_app();
    let webview = main_webview(&app);
    assert_eq!(state.presence_subscribers(), 0);
    // `onEvent` is `on_event` in Rust, and a channel travels as its callback id.
    let answer = call(
        &webview,
        "presence_events",
        json!({ "onEvent": "__CHANNEL__:7" }),
    )
    .unwrap();
    assert_eq!(answer, Value::Null);
    assert_eq!(state.presence_subscribers(), 1);
    // No channel, or something that is not one: refused, nothing registered.
    assert!(call(&webview, "presence_events", json!({})).is_err());
    assert!(call(&webview, "presence_events", json!({ "onEvent": 7 })).is_err());
    assert_eq!(state.presence_subscribers(), 1);
}

/// A message as the core hands it to the shell when it accepted a frame.
fn accepted_message() -> PresenceMessage {
    PresenceMessage {
        info: PresenceInfo {
            account_id: AccountId::from_bytes([1; 16]),
            device_id: DeviceId::from_bytes([2; 16]),
            name: "Ada".to_owned(),
            color: 3,
            page_id: Some(PageId::from_bytes([4; 16])),
            state: PresenceState::Editing,
            updated_at: 1_790_000_000_123,
        },
        update: vec![0, 1, 255],
    }
}

#[test]
fn an_accepted_frame_reaches_the_channel_as_the_json_the_transport_reads() {
    let (_app, state, _spy, _socket) = presence_app();
    let shared: SharedState = Arc::clone(&state) as SharedState;
    let received: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&received);
    let channel = tauri::ipc::Channel::<PresenceMessage>::new(move |body| {
        if let InvokeResponseBody::Json(json) = body {
            sink.lock().unwrap().push(json);
        }
        Ok(())
    });
    atomik_kb_desktop_lib::presence::subscribe(&shared, channel).unwrap();
    assert!(state.presence_deliver(&accepted_message()));
    let json: Value = serde_json::from_str(&received.lock().unwrap()[0]).unwrap();
    assert_eq!(
        json,
        json!({
            "info": {
                "accountId": serde_json::to_value(AccountId::from_bytes([1; 16])).unwrap(),
                "deviceId": serde_json::to_value(DeviceId::from_bytes([2; 16])).unwrap(),
                "name": "Ada",
                "color": 3,
                "pageId": serde_json::to_value(PageId::from_bytes([4; 16])).unwrap(),
                "state": "Editing",
                "updatedAt": 1_790_000_000_123_i64,
            },
            "update": [0, 1, 255],
        }),
        "an array of byte values, never a base64 string or a raw body"
    );
}

#[test]
fn a_page_that_reloads_loses_its_presence_channel_with_the_event_channels() {
    let (_app, state, _spy, _socket) = presence_app();
    let shared: SharedState = Arc::clone(&state) as SharedState;
    atomik_kb_desktop_lib::presence::subscribe(&shared, tauri::ipc::Channel::new(|_| Ok(())))
        .unwrap();
    assert_eq!(state.presence_subscribers(), 1);
    shared.clear_subscribers();
    assert_eq!(state.presence_subscribers(), 0);
    assert!(
        !state.presence_deliver(&accepted_message()),
        "nobody to hear it"
    );
    // A channel whose page is gone is forgotten at its first failed send.
    atomik_kb_desktop_lib::presence::subscribe(
        &shared,
        tauri::ipc::Channel::new(|_| Err(tauri::Error::Io(std::io::Error::other("gone")))),
    )
    .unwrap();
    assert!(!state.presence_deliver(&accepted_message()));
    assert_eq!(state.presence_subscribers(), 0);
}

#[test]
fn the_capability_adds_no_permission_for_presence() {
    // The commands are the app's own: `core:default` stays the main window's only permission.
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    assert_eq!(capability["permissions"], json!(["core:default"]));
    let description = capability["description"].as_str().unwrap();
    for command in ["presence_send", "presence_events"] {
        assert!(description.contains(command), "{command}");
    }
}
