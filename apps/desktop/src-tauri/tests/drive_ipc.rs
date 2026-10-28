//! The Google Drive commands of ADR-0059 (and `backup_folder_list`) through Tauri's real IPC layer, on the mock
//! runtime: they are registered under the documented names, take their arguments the way the webview's
//! `TauriDrive` sends them, answer what a core that is opening or locked answers before any dialog would open
//! (no test here may open a native dialog or reach Google: `drive_connect` is only ever called on a core that
//! is not ready), refuse the page's write of the setting the shell owns, and list a folder's backups. The flows
//! against a fake Google are the unit tests of `drive_host`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs::{self, File};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atomik_kb_desktop_lib::{AppState, CoreHost, MAIN_WINDOW, SharedState, configure, context};
use serde_json::{Value, json};
use tauri::http::HeaderMap;
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use common::Fake;

/// What a channel argument the webview sends carries.
const CHANNEL: &str = "__CHANNEL__:7";

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
            url: (if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" })
                .parse()
                .unwrap(),
            body: InvokeBody::Json(args),
            headers: HeaderMap::new(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body: InvokeResponseBody| body.deserialize::<Value>().unwrap())
}

/// The commands that answer a core that is not ready, with arguments the webview sends.
fn gated_commands() -> Vec<(&'static str, Value)> {
    vec![
        ("drive_connect", json!({})),
        ("drive_disconnect", json!({})),
        ("drive_client_set", json!({ "client": null })),
        (
            "drive_client_set",
            json!({ "client": { "clientId": "mine", "clientSecret": "s" } }),
        ),
        ("drive_backups_list", json!({})),
        (
            "drive_backup_download",
            json!({ "fileId": "abc", "onEvent": CHANNEL }),
        ),
        ("backup_folder_list", json!({})),
        ("backup_folder_list", json!({ "path": "C:\\Backups" })),
    ]
}

#[test]
fn a_core_that_is_still_opening_is_answered_before_any_dialog() {
    let state: SharedState = Arc::new(AppState::<kb_core::Core>::new());
    let app = app_over(state);
    let webview = main_webview(&app);
    for (command, args) in gated_commands() {
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
    let (state, world) = Fake::ready();
    CoreHost::lock(&*state).unwrap();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for (command, args) in gated_commands() {
        let error = call(&webview, command, args).unwrap_err();
        assert_eq!(
            error,
            json!({ "code": "Locked", "message": "the workspace is locked" }),
            "{command}"
        );
    }
    assert!(world.lock().unwrap().settings.is_empty());
}

#[test]
fn the_status_and_the_cancel_need_no_core() {
    for locked in [false, true] {
        let state: SharedState = if locked {
            let (state, _) = Fake::ready();
            CoreHost::lock(&*state).unwrap();
            state as SharedState
        } else {
            Arc::new(AppState::<kb_core::Core>::new()) as SharedState
        };
        let app = app_over(state);
        let webview = main_webview(&app);

        let status = call(&webview, "drive_status", json!({})).unwrap();
        assert_eq!(status["connected"], false);
        assert_eq!(status["needsReconnect"], false);
        assert!(
            status["client"] == "builtIn" || status["client"] == "none",
            "{status}"
        );
        assert_eq!(
            call(&webview, "drive_cancel", json!({})).unwrap(),
            Value::Null
        );
    }
}

#[test]
fn the_status_has_the_shape_the_page_reads() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().settings.insert(
        "app.backup.drive".to_owned(),
        json!({
            "account": "person@example.com",
            "folderId": "abc",
            "connectedAt": 1_700_000_000_000_i64,
            "ownClientId": "mine",
            "needsReconnect": true,
            "last": { "at": 5, "ok": false, "error": "InvalidInput: Google Drive is full" }
        }),
    );
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);

    let status = call(&webview, "drive_status", json!({})).unwrap();

    assert_eq!(
        status,
        json!({
            "client": "own",
            "connected": true,
            "account": "person@example.com",
            "folderUrl": "https://drive.google.com/drive/folders/abc",
            "ownClientId": "mine",
            "needsReconnect": true,
            "last": { "at": 5, "ok": false, "error": "InvalidInput: Google Drive is full" }
        })
    );
}

#[test]
fn the_page_cannot_write_the_drive_setting() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let error = call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": {
            "key": "app.backup.drive",
            "value": { "connectedAt": 1, "ownClientId": "evil" }
        } } }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "key");
    assert!(world.lock().unwrap().settings.is_empty());
}

#[test]
fn the_arguments_are_checked_before_anything_else() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for (command, args) in [
        // A channel the shell cannot read, or none; a file id that is not text.
        ("drive_backup_download", json!({ "fileId": "abc" })),
        ("drive_backup_download", json!({ "onEvent": CHANNEL })),
        (
            "drive_backup_download",
            json!({ "fileId": 5, "onEvent": CHANNEL }),
        ),
        // A client that is not an object, or has no id.
        ("drive_client_set", json!({ "client": "mine" })),
        (
            "drive_client_set",
            json!({ "client": { "clientSecret": "s" } }),
        ),
        ("backup_folder_list", json!({ "path": 5 })),
    ] {
        let error = call(&webview, command, args.clone()).unwrap_err();
        // Tauri answers a bad argument with text, not with an `ApiError`.
        assert!(error.is_string(), "{command} {args}: {error}");
    }
    assert!(world.lock().unwrap().log.is_empty());
}

#[test]
fn a_ready_core_with_no_connection_refuses_the_commands_that_need_one() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for (command, args) in [
        ("drive_backups_list", json!({})),
        (
            "drive_backup_download",
            json!({ "fileId": "abc", "onEvent": CHANNEL }),
        ),
    ] {
        let error = call(&webview, command, args).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{command}");
        assert_eq!(error["details"]["field"], "drive", "{command}");
        assert_eq!(error["details"]["reason"], "notConnected", "{command}");
    }
    // A file id that is no Drive id is refused first, naming the field.
    let error = call(
        &webview,
        "drive_backup_download",
        json!({ "fileId": "../x", "onEvent": CHANNEL }),
    )
    .unwrap_err();
    assert_eq!(error["details"]["field"], "fileId");
}

#[test]
fn an_own_client_is_checked_and_disconnecting_works_without_a_network() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().settings.insert(
        "app.backup.drive".to_owned(),
        json!({ "account": "p@example.com", "folderId": "abc", "connectedAt": 1, "ownClientId": "mine" }),
    );
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);

    let error = call(
        &webview,
        "drive_client_set",
        json!({ "client": { "clientId": "  " } }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "clientId");

    let status = call(&webview, "drive_disconnect", json!({})).unwrap();
    assert_eq!(status["connected"], false);
    assert_eq!(status["ownClientId"], "mine");
    assert_eq!(status["client"], "own");
    assert_eq!(
        world.lock().unwrap().settings["app.backup.drive"],
        json!({ "ownClientId": "mine", "needsReconnect": false })
    );
}

// --- backup_folder_list ---

fn write(path: &std::path::Path, size: usize, seconds: u64) {
    fs::write(path, vec![1_u8; size]).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn a_folder_given_by_path_is_listed_newest_first_in_the_shape_the_page_reads() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("old.atomikkb"), 10, 1_000);
    write(&dir.path().join("new.atomikkb"), 30, 3_000);
    write(&dir.path().join("notes.txt"), 5, 4_000);
    fs::create_dir(dir.path().join("sub.atomikkb")).unwrap();
    let folder = dir.path().to_string_lossy().into_owned();

    let listing = call(&webview, "backup_folder_list", json!({ "path": folder })).unwrap();

    assert_eq!(listing["folder"], folder.as_str());
    let backups = listing["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 2);
    assert_eq!(backups[0]["name"], "new.atomikkb");
    assert_eq!(backups[0]["size"], 30);
    assert_eq!(backups[0]["modifiedAt"], 3_000_000);
    assert_eq!(
        backups[0]["id"],
        dir.path().join("new.atomikkb").to_string_lossy().as_ref()
    );
    assert_eq!(backups[1]["name"], "old.atomikkb");
}

#[test]
fn a_folder_that_cannot_be_read_is_not_mistaken_for_a_cancelled_dialog() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    for path in [
        dir.path().join("missing").to_string_lossy().into_owned(),
        "relative/folder".to_owned(),
    ] {
        let error = call(&webview, "backup_folder_list", json!({ "path": path })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{path}");
        assert_eq!(error["details"]["field"], "path", "{path}");
        // The wizard takes "no file was chosen" for a cancel: this is another message.
        let message = error["message"].as_str().unwrap().to_lowercase();
        assert!(
            !message.starts_with("no file was chosen")
                && !message.starts_with("no folder was chosen"),
            "{message}"
        );
    }
}
