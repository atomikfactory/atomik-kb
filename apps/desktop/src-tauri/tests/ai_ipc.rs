//! The AI and MCP commands of ADR-0037 through Tauri's real IPC layer, on the mock runtime: they
//! are registered under the documented names, take their arguments the way the webview's
//! `TauriTransport` sends them, answer the shapes it validates, refuse while AI is off, while the core
//! is opening or locked (before any dialog would open: no test here may open a native dialog), and the
//! `dispatch` command refuses the requests only the shell makes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use atomik_kb_desktop_lib::{AppState, MAIN_WINDOW, SharedState, configure, context};
use axum::Router;
use axum::routing::get;
use kb_types::ai::{AI_ENDPOINT_SETTING, AI_SETTINGS_SETTING};
use serde_json::{Value, json};
use tauri::http::{HeaderMap, HeaderName, HeaderValue};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use common::Fake;

/// The header of `ai_transcribe`, spelled out here so a rename in the shell breaks this test (and the
/// TypeScript contract test) instead of passing silently.
const AI_HEADER: &str = "x-kb-ai";

/// The ids that a channel argument the webview sends would carry.
const CHANNEL: &str = "__CHANNEL__:7";

const COMMANDS: [&str; 9] = [
    "ai_status",
    "ai_endpoint_set",
    "ai_generate",
    "ai_chat",
    "ai_cancel",
    "ai_search",
    "ai_transcribe",
    "mcp_status",
    "mcp_client_grants_set",
];

fn app_over(state: SharedState) -> App<MockRuntime> {
    configure(mock_builder(), state).build(context()).unwrap()
}

fn main_webview(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .build()
        .unwrap()
}

fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    command: &str,
    body: InvokeBody,
    headers: &[(&str, &str)],
) -> Result<Value, Value> {
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
            url: (if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            })
            .parse()
            .unwrap(),
            body,
            headers: map,
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body: InvokeResponseBody| body.deserialize::<Value>().unwrap())
}

fn call(webview: &WebviewWindow<MockRuntime>, command: &str, args: Value) -> Result<Value, Value> {
    invoke(webview, command, InvokeBody::Json(args), &[])
}

fn starting() -> Value {
    json!({ "code": "Internal", "message": "core is starting" })
}

fn locked() -> Value {
    json!({ "code": "Locked", "message": "the workspace is locked" })
}

fn field_of(error: &Value) -> Option<&str> {
    error["details"]["field"].as_str()
}

fn put(world: &common::Shared, key: &str, value: Value) {
    world.lock().unwrap().settings.insert(key.to_owned(), value);
}

/// Every command with the arguments the transport sends, as far as they can be written without a
/// channel (`ai_generate` and `ai_chat` get one below).
fn json_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("ai_status", json!({})),
        (
            "ai_endpoint_set",
            json!({ "url": "http://127.0.0.1:11434" }),
        ),
        ("ai_cancel", json!({ "requestId": "r1" })),
        (
            "ai_search",
            json!({ "query": "launch", "scope": { "type": "all" }, "limit": 6 }),
        ),
        ("mcp_status", json!({})),
        (
            "mcp_client_grants_set",
            json!({ "clientId": "c".repeat(32), "grants": [] }),
        ),
    ]
}

fn stream_calls() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ai_generate",
            json!({
                "requestId": "g1",
                "input": { "action": "improve", "text": "x" },
                "onEvent": CHANNEL,
            }),
        ),
        (
            "ai_chat",
            json!({
                "requestId": "c1",
                "input": { "messages": [{ "role": "user", "content": "q?" }], "scope": { "type": "all" } },
                "onEvent": CHANNEL,
            }),
        ),
    ]
}

#[test]
fn every_ai_and_mcp_command_is_registered() {
    let app = app_over(Arc::new(AppState::<kb_core::Core>::new()));
    let webview = main_webview(&app);
    for command in COMMANDS {
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
fn a_core_that_is_still_opening_answers_before_anything_else() {
    let app = app_over(Arc::new(AppState::<kb_core::Core>::new()));
    let webview = main_webview(&app);
    for (command, args) in json_calls().into_iter().chain(stream_calls()) {
        if command == "ai_cancel" {
            // Cancelling needs no core: an id that is not running is fine.
            assert_eq!(
                call(&webview, command, args).unwrap(),
                Value::Null,
                "{command}"
            );
            continue;
        }
        assert_eq!(
            call(&webview, command, args).unwrap_err(),
            starting(),
            "{command}"
        );
    }
    let wav = InvokeBody::Raw(vec![0; 64]);
    assert_eq!(
        invoke(&webview, "ai_transcribe", wav, &[]).unwrap_err(),
        starting()
    );
}

#[test]
fn a_locked_core_answers_locked_and_opens_no_dialog() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();
    // A remote endpoint and a grants change would open a native dialog on a ready core.
    let mut calls = json_calls();
    calls.push(("ai_endpoint_set", json!({ "url": "http://nas:11434" })));
    calls.push((
        "mcp_client_grants_set",
        json!({ "clientId": "c".repeat(32), "grants": [{ "spaceId": "09090909-0909-0909-0909-090909090909", "read": true, "write": true }] }),
    ));
    for (command, args) in calls.into_iter().chain(stream_calls()) {
        if command == "ai_cancel" {
            continue;
        }
        assert_eq!(
            call(&webview, command, args).unwrap_err(),
            locked(),
            "{command}"
        );
    }
    assert_eq!(
        invoke(&webview, "ai_transcribe", InvokeBody::Raw(vec![0; 64]), &[]).unwrap_err(),
        locked()
    );
}

#[test]
fn with_ai_off_everything_but_the_status_and_the_endpoint_refuses_for_the_field_enabled() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for (command, args) in [(
        "ai_search",
        json!({ "query": "launch", "scope": { "type": "all" } }),
    )]
    .into_iter()
    .chain(stream_calls())
    {
        let error = call(&webview, command, args).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{command}");
        assert_eq!(field_of(&error), Some("enabled"), "{command}");
        assert_eq!(error["message"], "AI is turned off", "{command}");
    }
    let error = invoke(
        &webview,
        "ai_transcribe",
        InvokeBody::Raw(vec![0; 64]),
        &[(AI_HEADER, "{}")],
    )
    .unwrap_err();
    assert_eq!(field_of(&error), Some("enabled"));
    // The core never saw a retrieval.
    assert!(
        !world
            .lock()
            .unwrap()
            .log
            .iter()
            .any(|kind| kind == "AiRetrieve")
    );
}

#[test]
fn ai_transcribe_checks_its_header_and_the_size_of_its_body() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    put(&world, AI_SETTINGS_SETTING, json!({ "enabled": true }));
    // A header that is not JSON.
    let error = invoke(
        &webview,
        "ai_transcribe",
        InvokeBody::Raw(vec![0; 64]),
        &[(AI_HEADER, "not json")],
    )
    .unwrap_err();
    assert_eq!(
        (error["code"].as_str(), field_of(&error)),
        (Some("InvalidInput"), Some(AI_HEADER))
    );
    // A body of more than 4 MiB is refused for the field audio, whatever else is wrong.
    let error = invoke(
        &webview,
        "ai_transcribe",
        InvokeBody::Raw(vec![0; 4 * 1024 * 1024 + 1]),
        &[(AI_HEADER, "{\"model\":\"r2t2\"}")],
    )
    .unwrap_err();
    assert_eq!(field_of(&error), Some("audio"));
    assert_eq!(error["details"]["limit"], 4 * 1024 * 1024);
    // Something that is not a WAV file, with AI on and a model named by the header.
    let error = invoke(
        &webview,
        "ai_transcribe",
        InvokeBody::Raw(b"not a wav file".to_vec()),
        &[(AI_HEADER, "{\"model\":\"r2t2\"}")],
    )
    .unwrap_err();
    assert_eq!(field_of(&error), Some("audio"));
    // No body at all is the same.
    let error = invoke(&webview, "ai_transcribe", InvokeBody::default(), &[]).unwrap_err();
    assert_eq!(field_of(&error), Some("audio"));
}

#[test]
fn the_endpoint_status_has_the_shape_the_transport_validates() {
    // A stub that is just enough of Ollama for the status.
    let addr = tauri::async_runtime::block_on(async {
        let app = Router::new()
            .route("/api/version", get(|| async { axum::Json(json!({ "version": "0.35.0" })) }))
            .route(
                "/api/tags",
                get(|| async {
                    axum::Json(json!({ "models": [
                        { "name": "qwen3:8b", "size": 5_200_000_000_u64, "details": { "family": "qwen3", "parameter_size": "8.2B" } },
                        { "name": "nomic-embed-text:latest", "size": 274_000_000_u64, "details": { "family": "nomic-bert", "parameter_size": "137M" } },
                    ] }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    });
    let url = format!("http://{addr}");

    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let status = call(
        &webview,
        "ai_endpoint_set",
        json!({ "url": format!("{url}/") }),
    )
    .unwrap();
    assert_eq!(status["endpoint"], url);
    assert_eq!(status["local"], true);
    assert_eq!(status["remoteConsent"], false);
    assert_eq!(status["reachable"], true);
    assert_eq!(status["version"], "0.35.0");
    assert_eq!(status["models"].as_array().unwrap().len(), 2);
    let model = &status["models"][0];
    assert_eq!(model["name"], "qwen3:8b");
    assert_eq!(model["family"], "qwen3");
    assert_eq!(model["parameterSize"], "8.2B");
    assert!(model["capabilities"].is_array());
    assert_eq!(model["size"], 5_200_000_000_u64);
    assert_eq!(status["indexer"]["phase"], "idle");
    assert!(status.get("error").is_none() || status["error"].is_null());
    // The endpoint is stored through the shell's own handle on the core, with no consent.
    assert_eq!(
        world.lock().unwrap().settings[AI_ENDPOINT_SETTING],
        json!({ "url": url, "remoteConsent": false })
    );
    // And the status command says the same.
    let again = call(&webview, "ai_status", json!({})).unwrap();
    assert_eq!(again["endpoint"], url);
    assert_eq!(again["reachable"], true);

    // What is not an endpoint is refused for the field url, before any dialog.
    for bad in ["ftp://nas", "http://user:pw@nas", "http://nas/api", ""] {
        let error = call(&webview, "ai_endpoint_set", json!({ "url": bad })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{bad}");
        assert_eq!(field_of(&error), Some("url"), "{bad}");
    }
}

#[test]
fn the_mcp_status_has_the_shape_the_transport_validates() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let status = call(&webview, "mcp_status", json!({})).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["running"], false);
    assert_eq!(status["port"], 47_811);
    assert_eq!(status["url"], "http://127.0.0.1:47811/mcp");
    put(
        &world,
        "app.mcp.settings",
        json!({ "enabled": false, "port": 50123 }),
    );
    let status = call(&webview, "mcp_status", json!({})).unwrap();
    assert_eq!(
        (status["port"].as_u64(), status["url"].as_str()),
        (Some(50_123), Some("http://127.0.0.1:50123/mcp"))
    );
}

#[test]
fn the_webview_cannot_send_what_only_the_shell_makes() {
    let (state, world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    for request in [
        json!({ "type": "AiIndexNext", "data": { "maxChunks": 16 } }),
        json!({ "type": "AiIndexStore", "data": { "pageId": "09090909-0909-0909-0909-090909090909", "sourceHash": "00".repeat(32), "model": "m", "vectors": [] } }),
        json!({ "type": "McpCall", "data": { "token": "kbm_secret", "message": "{}" } }),
        json!({ "type": "McpClientGrantsSet", "data": { "clientId": "c".repeat(32), "grants": [] } }),
    ] {
        let error = call(&webview, "dispatch", json!({ "request": request })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{request}");
        assert_eq!(field_of(&error), Some("type"), "{request}");
        assert!(!error.to_string().contains("secret"), "{error}");
    }
    // A write to the endpoint (and its consent) is refused as well; the settings the UI writes pass.
    let error = call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": { "key": AI_ENDPOINT_SETTING, "value": { "url": "https://evil.example", "remoteConsent": true } } } }),
    )
    .unwrap_err();
    assert_eq!(field_of(&error), Some("key"));
    call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": { "key": AI_SETTINGS_SETTING, "value": { "enabled": true } } } }),
    )
    .unwrap();
    call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "AiIndexClear" } }),
    )
    .unwrap();
    // The core saw the two requests that were allowed, and nothing else.
    let log = world.lock().unwrap().log.clone();
    assert_eq!(log, ["SettingsSet", "AiIndexClear"]);
    assert!(
        !world
            .lock()
            .unwrap()
            .settings
            .contains_key(AI_ENDPOINT_SETTING)
    );
}

#[test]
fn the_main_window_has_no_more_permissions_for_the_new_commands() {
    // The capability grants `core:default` and nothing else; the commands need no entries, and the
    // webview still cannot call the plugins' own commands (dialog, opener, notification).
    let capability = include_str!("../capabilities/default.json");
    let parsed: Value = serde_json::from_str(capability).unwrap();
    assert_eq!(parsed["permissions"], json!(["core:default"]));
    for command in COMMANDS {
        assert!(
            parsed["description"].as_str().unwrap().contains(command),
            "{command} is not in the capability's description"
        );
    }
}
