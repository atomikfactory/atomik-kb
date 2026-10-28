//! Deep links (ADR-0040) through Tauri's real IPC layer, on the mock runtime: the `shell_events_subscribe`
//! command (the channel that carries the shell's messages, the links among them), the links that reach it (and
//! the ones that do not), the deep-link plugin's event feeding the shell, and the promise that the webview can
//! reach none of the plugins' commands. The other messages of the channel are in `desktop.rs`.
//!
//! What this cannot run is the single-instance plugin itself: a second instance forwards its arguments and
//! then ends the process with `std::process::exit`, which would end the test run, and its window class and
//! named mutex belong to the real runtime. Its callback is the shell's `on_second_launch`, tested here up to
//! the window it focuses; the plugin's own forwarding (and the OS registration of the scheme) is checked in the
//! real-app pass (`docs/development/packaging.md`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use atomik_kb_desktop_lib::deep_link;
use atomik_kb_desktop_lib::{
    AppState, MAIN_WINDOW, SharedState, ShellEvents, configure, context, window,
};
use serde_json::{Value, json};
use tauri::http::HeaderMap;
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const PAGE: &str = "12345678-9abc-4def-8123-456789abcdef";
const BLOCK: &str = "0a1b2c3d-4e5f-4a6b-8c7d-8e9f0a1b2c3d";
const OTHER: &str = "87654321-cba9-4fed-8321-fedcba987654";

/// What the shell sent to channels, as `(callback id, JSON message)`.
type Sent = Arc<Mutex<Vec<(u32, Value)>>>;

/// An app with the shell's commands, state and plugins on the mock runtime and the real `tauri.conf.json`,
/// whose channels record what is sent to them instead of running JavaScript.
fn app() -> (App<MockRuntime>, Sent) {
    let sent: Sent = Arc::default();
    let sink = Arc::clone(&sent);
    let builder = mock_builder().channel_interceptor(move |_webview, callback, _index, body| {
        if let InvokeResponseBody::Json(text) = body {
            sink.lock()
                .unwrap()
                .push((callback.0, serde_json::from_str(text).unwrap()));
        }
        true
    });
    let state: SharedState = Arc::new(AppState::<kb_core::Core>::new());
    let app = configure(builder, state).build(context()).unwrap();
    (app, sent)
}

fn main_webview(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .build()
        .unwrap()
}

/// One IPC message as the webview's `invoke` sends it, from the app's own origin.
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

fn links(app: &App<MockRuntime>) -> Arc<ShellEvents> {
    Arc::clone(app.state::<Arc<ShellEvents>>().inner())
}

/// Hands a URL to the shell as the OS does; whether the shell accepted it.
fn offer(app: &App<MockRuntime>, url: &str) -> bool {
    deep_link::offer(&links(app), url)
}

/// A link message as the webview reads it.
fn message(data: Value) -> Value {
    json!({ "type": "deepLink", "data": data })
}

/// The channel argument the webview's `new Channel()` becomes in a call, with its callback id.
fn channel(id: u32) -> Value {
    json!(format!("__CHANNEL__:{id}"))
}

fn page_url() -> String {
    format!("atomik-kb://page/{PAGE}")
}

fn block_url() -> String {
    format!("atomik-kb://page/{OTHER}#{BLOCK}")
}

// --- the command ---

#[test]
fn shell_events_subscribe_is_registered_and_needs_its_channel() {
    let (app, _) = app();
    let webview = main_webview(&app);
    assert_eq!(
        call(
            &webview,
            "shell_events_subscribe",
            json!({ "onEvent": channel(1) })
        ),
        Ok(Value::Null)
    );
    // No channel, or something that is not one: refused by argument extraction, nothing subscribed.
    for args in [
        json!({}),
        json!({ "onEvent": "not a channel" }),
        json!({ "onEvent": 5 }),
    ] {
        let error = call(&webview, "shell_events_subscribe", args.clone()).unwrap_err();
        let message = error.as_str().unwrap_or_default();
        assert!(
            message.contains("onEvent") || message.contains("channel"),
            "{args}: {error}"
        );
    }
}

#[test]
fn a_link_reaches_the_subscribed_channel_as_the_ui_reads_it() {
    let (app, sent) = app();
    let webview = main_webview(&app);
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(7) }),
    )
    .unwrap();

    assert!(offer(&app, &page_url()));
    assert!(offer(&app, &block_url()));

    assert_eq!(
        *sent.lock().unwrap(),
        vec![
            (7, message(json!({ "pageId": PAGE }))),
            (7, message(json!({ "pageId": OTHER, "blockId": BLOCK }))),
        ]
    );
    assert!(!links(&app).has_pending());
}

#[test]
fn a_link_from_before_the_page_subscribed_arrives_with_the_subscription() {
    // The cold start: the OS gave the app the link while its page was still loading.
    let (app, sent) = app();
    let webview = main_webview(&app);
    assert!(offer(&app, &block_url()));
    assert!(links(&app).has_pending());
    assert!(sent.lock().unwrap().is_empty());

    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(3) }),
    )
    .unwrap();

    assert_eq!(
        *sent.lock().unwrap(),
        vec![(3, message(json!({ "pageId": OTHER, "blockId": BLOCK })))]
    );
    assert!(!links(&app).has_pending());
}

#[test]
fn what_is_not_a_page_link_never_reaches_the_channel() {
    let (app, sent) = app();
    let webview = main_webview(&app);
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(5) }),
    )
    .unwrap();

    for text in [
        "atomik-kb://pair?server=https%3A%2F%2Fexample.com&code=ABCD".to_owned(),
        format!("{}?x=1", page_url()),
        format!("{}/", page_url()),
        format!("https://example.com/{PAGE}"),
        "C:\\Windows\\notepad.exe".to_owned(),
        String::new(),
    ] {
        assert!(!offer(&app, &text), "{text}");
    }
    assert!(sent.lock().unwrap().is_empty());
    assert!(!links(&app).has_pending());
}

// --- invitation links ---

/// The secret of an invite: 52 letters and digits.
const SECRET: &str = "8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB";

fn invite_url(server: &str) -> String {
    format!("atomik-kb://invite?server={server}#{SECRET}")
}

/// An invitation message as the webview reads it.
fn invite_message(link: &str) -> Value {
    json!({ "type": "inviteLink", "data": { "link": link } })
}

#[test]
fn an_invitation_reaches_the_subscribed_channel_as_the_ui_reads_it() {
    let (app, sent) = app();
    let webview = main_webview(&app);
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(11) }),
    )
    .unwrap();

    let invitation = invite_url("http%3A%2F%2F127.0.0.1%3A8787");
    assert!(offer(&app, &invitation));
    // A page link in between does not disturb it, and the two kinds keep their own messages.
    assert!(offer(&app, &page_url()));

    assert_eq!(
        *sent.lock().unwrap(),
        vec![
            (11, invite_message(&invitation)),
            (11, message(json!({ "pageId": PAGE }))),
        ]
    );
    assert!(!links(&app).has_pending());
}

#[test]
fn an_invitation_from_before_the_page_subscribed_arrives_with_the_subscription() {
    // The cold start: the OS gave the app the link while its page was still loading.
    let (app, sent) = app();
    let webview = main_webview(&app);
    let first = invite_url("http%3A%2F%2F127.0.0.1%3A8787");
    let second = invite_url("https%3A%2F%2Fsync.example.com");
    assert!(offer(&app, &first));
    assert!(offer(&app, &second));
    assert!(links(&app).has_pending());
    assert!(sent.lock().unwrap().is_empty());

    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(12) }),
    )
    .unwrap();

    // The last link won.
    assert_eq!(*sent.lock().unwrap(), vec![(12, invite_message(&second))]);
    assert!(!links(&app).has_pending());

    // A page that reloads subscribes again and is told nothing old.
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(13) }),
    )
    .unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1);
}

#[test]
fn what_is_not_an_invitation_never_reaches_the_channel() {
    let (app, sent) = app();
    let webview = main_webview(&app);
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(14) }),
    )
    .unwrap();

    for text in [
        // A secret of the wrong length, an address that is not escaped, another scheme for the address,
        // a second parameter, no fragment.
        invite_url("http%3A%2F%2F127.0.0.1").replace(SECRET, "SHORT"),
        invite_url("http://127.0.0.1"),
        invite_url("ftp%3A%2F%2Fexample.com"),
        format!("{}&x=1", invite_url("http%3A%2F%2F127.0.0.1")).replace('#', "&y=2#"),
        "atomik-kb://invite?server=http%3A%2F%2F127.0.0.1".to_owned(),
        format!("https://sync.example.com/join#{SECRET}"),
        format!("{} ", invite_url("http%3A%2F%2F127.0.0.1")),
    ] {
        assert!(!offer(&app, &text), "{text}");
    }
    assert!(sent.lock().unwrap().is_empty());
    assert!(!links(&app).has_pending());
}

#[test]
fn the_plugin_s_event_carries_invitations_too() {
    let (app, sent) = app();
    let webview = main_webview(&app);
    deep_link::listen(&app);
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(15) }),
    )
    .unwrap();

    let invitation = invite_url("http%3A%2F%2F127.0.0.1%3A8787");
    app.emit(
        "deep-link://new-url",
        vec![
            invitation.clone(),
            "atomik-kb://invite?server=x#y".to_owned(),
        ],
    )
    .unwrap();

    assert_eq!(
        *sent.lock().unwrap(),
        vec![(15, invite_message(&invitation))]
    );
}

// --- the deep-link plugin feeding the shell ---

#[test]
fn the_plugin_s_event_is_what_the_shell_listens_to() {
    // `tauri-plugin-single-instance` makes the deep-link plugin raise this event for the arguments of a
    // second launch (and macOS for a link the OS opens): the shell turns each URL into an offer.
    let (app, sent) = app();
    let webview = main_webview(&app);
    deep_link::listen(&app);
    call(
        &webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(9) }),
    )
    .unwrap();

    app.emit("deep-link://new-url", vec![page_url(), block_url()])
        .unwrap();
    // Not links of this item, in the same event: ignored one by one.
    app.emit(
        "deep-link://new-url",
        vec![
            "atomik-kb://pair?server=x&code=y".to_owned(),
            format!("{}#nope", page_url()),
        ],
    )
    .unwrap();

    assert_eq!(
        *sent.lock().unwrap(),
        vec![
            (9, message(json!({ "pageId": PAGE }))),
            (9, message(json!({ "pageId": OTHER, "blockId": BLOCK }))),
        ]
    );
}

#[test]
fn a_launch_without_a_link_holds_nothing() {
    // `get_current` of the plugin is the process's own arguments: a test run has no link among them.
    let (app, _) = app();
    deep_link::listen(&app);
    assert!(!links(&app).has_pending());
}

// --- the window ---

#[test]
fn a_second_launch_has_no_window_to_show_until_there_is_one() {
    let (app, _) = app();
    assert!(!window::show_main_window(&app));
    deep_link::on_second_launch(app.handle());

    let webview = main_webview(&app);
    assert!(window::show_main_window(&app));
    deep_link::on_second_launch(app.handle());
    assert_eq!(webview.label(), MAIN_WINDOW);
}

// --- what the webview cannot do ---

#[test]
fn the_webview_cannot_call_the_deep_link_plugin_directly() {
    let (app, _) = app();
    let webview = main_webview(&app);
    // `register` and `unregister` would write and delete the scheme's registration; the arguments are
    // nonsense on purpose, so that a call that was ever allowed would fail to deserialise instead.
    for (command, args) in [
        ("plugin:deep-link|get_current", json!({})),
        ("plugin:deep-link|is_registered", json!({ "protocol": 5 })),
        ("plugin:deep-link|register", json!({ "protocol": 5 })),
        ("plugin:deep-link|unregister", json!({ "protocol": 5 })),
    ] {
        let error = call(&webview, command, args).unwrap_err();
        let message = error.as_str().unwrap_or_default();
        assert!(message.contains("not allowed"), "{command}: {error}");
    }
}

#[test]
fn the_deep_link_plugin_is_configured_for_the_scheme_and_nothing_else() {
    let (app, _) = app();
    let plugins = &app.config().plugins.0;
    assert_eq!(
        plugins.get("deep-link").cloned(),
        Some(json!({ "desktop": { "schemes": [deep_link::SCHEME] } }))
    );
    assert_eq!(deep_link::SCHEME, "atomik-kb");
}
