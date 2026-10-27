//! The commands of ADR-0036 through Tauri's real IPC layer, on the mock runtime: the arguments
//! the webview sends, the refusal of a write to a setting only the shell may write, and the promise
//! that a locked or still-opening core is answered before any dialog would open (no test here may
//! open a native dialog: every call that reaches a dialog is a bug in the test).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use atomik_kb_desktop_lib::{
    AppState, MAIN_WINDOW, SharedState, configure, context, exporter::ExportRunResult,
};
use kb_types::{ExportWarning, ExportWarningCode};
use serde_json::{Value, json};
use tauri::http::HeaderMap;
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use common::Fake;

const SPACE: &str = "09090909-0909-0909-0909-090909090909";

fn app_over(state: SharedState) -> App<MockRuntime> {
    configure(mock_builder(), state).build(context()).unwrap()
}

fn main_webview(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .build()
        .unwrap()
}

fn call(webview: &WebviewWindow<MockRuntime>, command: &str, args: Value) -> Result<Value, Value> {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: HeaderMap::new(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body: InvokeResponseBody| body.deserialize::<Value>().unwrap())
}

/// The five commands with arguments the webview's `TauriTransport` sends.
fn interop_commands() -> [(&'static str, Value); 5] {
    [
        (
            "export_run",
            json!({ "options": { "spaceId": SPACE, "includeChildren": false, "format": "markdown" } }),
        ),
        (
            "import_run",
            json!({ "options": { "spaceId": SPACE, "source": "folder" } }),
        ),
        (
            "save_text_file",
            json!({ "suggestedName": "Tasks.csv", "text": "Name\r\n" }),
        ),
        ("backup_folder_choose", json!({})),
        ("backup_run_now", json!({})),
    ]
}

#[test]
fn a_core_that_is_still_opening_is_answered_before_any_dialog() {
    let state: SharedState = Arc::new(AppState::<kb_core::Core>::new());
    let app = app_over(state);
    let webview = main_webview(&app);
    for (command, args) in interop_commands() {
        let error = call(&webview, command, args).unwrap_err();
        assert_eq!(
            error,
            json!({ "code": "Internal", "message": "core is starting" }),
            "{command}"
        );
    }
}

#[test]
fn a_locked_core_is_answered_before_any_dialog() {
    let (state, _world) = Fake::ready();
    atomik_kb_desktop_lib::CoreHost::lock(&*state).unwrap();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for (command, args) in interop_commands() {
        let error = call(&webview, command, args).unwrap_err();
        assert_eq!(
            error,
            json!({ "code": "Locked", "message": "the workspace is locked" }),
            "{command}"
        );
    }
}

#[test]
fn the_arguments_are_checked_before_anything_else() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for (command, args) in [
        ("export_run", json!({})),
        (
            "export_run",
            json!({ "options": { "spaceId": "not-an-id", "includeChildren": true, "format": "markdown" } }),
        ),
        (
            "export_run",
            json!({ "options": { "spaceId": SPACE, "includeChildren": true, "format": "pdf" } }),
        ),
        ("import_run", json!({})),
        (
            "import_run",
            json!({ "options": { "spaceId": SPACE, "source": "cloud" } }),
        ),
        ("import_run", json!({ "options": { "spaceId": SPACE } })),
        ("save_text_file", json!({ "suggestedName": "x.csv" })),
        ("save_text_file", json!({ "text": "x" })),
    ] {
        let error = call(&webview, command, args.clone()).unwrap_err();
        // Tauri answers a bad argument with text, not with an `ApiError`, and the core is not
        // asked anything (a ready core would otherwise open a dialog).
        assert!(error.is_string(), "{command} {args}: {error}");
    }
    assert!(world.lock().unwrap().log.is_empty());
}

#[test]
fn the_webview_cannot_write_the_settings_the_shell_owns() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);

    for key in ["app.backup.folder", "app.backup.last"] {
        let error = call(
            &webview,
            "dispatch",
            json!({ "request": { "type": "SettingsSet", "data": { "key": key, "value": "C:\\Elsewhere" } } }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{key}");
        assert_eq!(error["details"]["field"], "key", "{key}");
    }
    assert!(world.lock().unwrap().settings.is_empty());
    assert!(world.lock().unwrap().log.is_empty());

    // Every other setting, including the schedule the settings screen writes, goes through, and
    // the two shell-owned ones can be read.
    for (key, value) in [
        (
            "app.backup.schedule",
            json!({ "enabled": true, "interval": "daily", "keep": 7 }),
        ),
        ("ui.theme", json!("dark")),
    ] {
        call(
            &webview,
            "dispatch",
            json!({ "request": { "type": "SettingsSet", "data": { "key": key, "value": value } } }),
        )
        .unwrap();
        assert_eq!(world.lock().unwrap().settings.get(key), Some(&value));
    }
    for key in ["app.backup.folder", "app.backup.last"] {
        let answer = call(
            &webview,
            "dispatch",
            json!({ "request": { "type": "SettingsGet", "data": { "key": key } } }),
        )
        .unwrap();
        assert_eq!(answer["type"], "Setting", "{key}");
    }
}

#[test]
fn the_shell_writes_the_settings_it_owns_through_its_own_handle() {
    let (state, world) = Fake::ready();
    let folder = tempfile::tempdir().unwrap();
    // Not through the `dispatch` command: the shell's handle on the state.
    atomik_kb_desktop_lib::backup_scheduler::choose_folder(&*state, || {
        Ok(folder.path().to_path_buf())
    })
    .unwrap();
    assert!(
        world
            .lock()
            .unwrap()
            .settings
            .contains_key("app.backup.folder")
    );
}

#[test]
fn what_export_run_answers_has_the_shape_the_transport_validates() {
    let result = ExportRunResult {
        path: "C:\\x\\Space.zip".to_owned(),
        pages: 3,
        attachments: 2,
        warnings: vec![ExportWarning {
            code: ExportWarningCode::AttachmentUnreadable,
            page_id: None,
            attachment_id: Some(kb_types::AttachmentId::from_bytes([4; 16])),
        }],
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["path"], "C:\\x\\Space.zip");
    assert_eq!(
        (json["pages"].clone(), json["attachments"].clone()),
        (json!(3), json!(2))
    );
    assert_eq!(json["warnings"][0]["code"], "attachmentUnreadable");
    assert!(json["warnings"][0]["attachmentId"].is_string());
    // `warnings` is always there, even when empty.
    let empty = ExportRunResult {
        warnings: Vec::new(),
        ..result
    };
    assert_eq!(serde_json::to_value(&empty).unwrap()["warnings"], json!([]));
}
