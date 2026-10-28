//! The desktop shell's tray and quick capture (ADR-0040) through Tauri's real IPC layer, on the mock runtime:
//! the messages the shell sends the webview (`capture`, `lock`, the shortcut's state), the labels the UI sends
//! (`shell_labels_set`), what each tray action and the shortcut do, and how the shell follows the two device
//! settings (`app.tray.close_to_tray`, `app.shortcut.capture`) through the core's events.
//!
//! What the mock runtime cannot run: the operating system's notification area and hotkey table. The tray
//! icon, its menu, the click on it, the registration of the shortcut with Windows and the window event that
//! turns a close into a hide are not reachable here (the mock runtime has no window event listeners, and
//! `request_exit` is `unimplemented!()`). What is tested instead is everything around them: the pure rules
//! (`accelerator`, `window::close_decision` and `on_close_requested`, `shortcut::Controller` over a fake
//! registrar), the actions the tray menu and the shortcut call, the messages, the labels, and the worker over
//! the scripted core. The tray, the shortcut, the hide and the quit are checked in the real-app pass.
//!
//! The test that builds the tray (`the_tray_takes_its_labels_from_the_store_and_follows_the_ui`) does so on the
//! mock runtime, which still creates the real notification-area icon through the `tray-icon` crate; it is
//! skipped when the session has none.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atomik_kb_desktop_lib::accelerator::Accelerator;
use atomik_kb_desktop_lib::desktop::{self, SettingsWorker};
use atomik_kb_desktop_lib::desktop_settings::{
    CAPTURE_SHORTCUT_SETTING, CLOSE_TO_TRAY_SETTING, DEFAULT_CAPTURE_SHORTCUT,
};
use atomik_kb_desktop_lib::shell_labels::Labels;
use atomik_kb_desktop_lib::shortcut::{Controller, Registrar, ShortcutState};
use atomik_kb_desktop_lib::tray::TrayAction;
use atomik_kb_desktop_lib::{
    AppState, CoreHost, DesktopPrefs, LabelStore, MAIN_WINDOW, SharedState, ShellEvents, configure,
    context,
};
use common::{Fake, Shared};
use serde_json::{Value, json};
use tauri::http::HeaderMap;
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{ShortcutEvent, ShortcutState as KeyState};

/// What the shell sent to channels, as `(callback id, JSON message)`.
type Sent = Arc<Mutex<Vec<(u32, Value)>>>;

struct Harness {
    app: App<MockRuntime>,
    sent: Sent,
    state: Arc<AppState<Fake>>,
    world: Shared,
}

/// An app with the shell's commands, state and plugins on the mock runtime and the real `tauri.conf.json`, over
/// a scripted core that is ready, whose channels record what is sent to them.
fn harness() -> Harness {
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
    let (state, world) = Fake::ready();
    let shared: SharedState = Arc::clone(&state) as SharedState;
    let app = configure(builder, shared).build(context()).unwrap();
    Harness {
        app,
        sent,
        state,
        world,
    }
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
            url: (if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            })
            .parse()
            .unwrap(),
            body: InvokeBody::Json(args),
            headers: HeaderMap::new(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body: InvokeResponseBody| body.deserialize::<Value>().unwrap())
}

/// The channel argument the webview's `new Channel()` becomes in a call, with its callback id.
fn channel(id: u32) -> Value {
    json!(format!("__CHANNEL__:{id}"))
}

fn events(app: &App<MockRuntime>) -> Arc<ShellEvents> {
    Arc::clone(app.state::<Arc<ShellEvents>>().inner())
}

fn prefs(app: &App<MockRuntime>) -> Arc<DesktopPrefs> {
    Arc::clone(app.state::<Arc<DesktopPrefs>>().inner())
}

fn subscribe(webview: &WebviewWindow<MockRuntime>, id: u32) {
    call(
        webview,
        "shell_events_subscribe",
        json!({ "onEvent": channel(id) }),
    )
    .unwrap();
}

fn press() -> ShortcutEvent {
    ShortcutEvent {
        id: 1,
        state: KeyState::Pressed,
    }
}

fn release() -> ShortcutEvent {
    ShortcutEvent {
        id: 1,
        state: KeyState::Released,
    }
}

// --- the channel: capture, lock, the shortcut's state ---

#[test]
fn a_capture_reaches_the_subscribed_channel() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 4);

    desktop::capture(harness.app.handle());

    assert_eq!(
        *harness.sent.lock().unwrap(),
        vec![(4, json!({ "type": "capture" }))]
    );
}

#[test]
fn a_capture_from_before_the_page_subscribed_arrives_with_the_subscription() {
    // The shortcut was pressed while the page was still loading, with the window not even created.
    let harness = harness();
    desktop::capture(harness.app.handle());
    desktop::capture(harness.app.handle());
    assert!(events(&harness.app).has_pending());

    let webview = main_webview(&harness.app);
    subscribe(&webview, 6);

    assert_eq!(
        *harness.sent.lock().unwrap(),
        vec![(6, json!({ "type": "capture" }))]
    );
    assert!(!events(&harness.app).has_pending());
}

#[test]
fn the_tray_s_new_page_and_the_shortcut_both_make_a_capture() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 8);

    desktop::perform(harness.app.handle(), TrayAction::NewPage);
    desktop::on_shortcut_event(harness.app.handle(), &press());

    let capture = json!({ "type": "capture" });
    assert_eq!(
        *harness.sent.lock().unwrap(),
        vec![(8, capture.clone()), (8, capture)]
    );
}

#[test]
fn releasing_the_shortcut_makes_nothing() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 8);

    desktop::on_shortcut_event(harness.app.handle(), &release());

    assert!(harness.sent.lock().unwrap().is_empty());
}

#[test]
fn open_shows_the_window_and_tells_the_page_nothing() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 8);

    desktop::perform(harness.app.handle(), TrayAction::Open);

    assert!(webview.is_visible().unwrap());
    assert!(harness.sent.lock().unwrap().is_empty());
}

#[test]
fn the_tray_s_lock_asks_the_page_to_run_its_own_lock_action() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 3);

    desktop::perform(harness.app.handle(), TrayAction::Lock);

    assert_eq!(
        *harness.sent.lock().unwrap(),
        vec![(3, json!({ "type": "lock" }))]
    );
    // The page locks the core (after sending the editor's text); the shell did not.
    assert_eq!(harness.state.state_info(), ready());
}

fn ready() -> atomik_kb_desktop_lib::CoreStateInfo {
    atomik_kb_desktop_lib::CoreStateInfo::Ready
}

#[test]
fn the_tray_s_lock_locks_the_core_itself_when_no_page_can_be_asked() {
    let harness = harness();
    // No subscriber at all: the page is still loading.
    desktop::perform(harness.app.handle(), TrayAction::Lock);
    wait_until("the core is locked", || {
        harness.state.state_info() == atomik_kb_desktop_lib::CoreStateInfo::Locked
    });
    // A lock is not held for a page that subscribes later.
    let webview = main_webview(&harness.app);
    subscribe(&webview, 3);
    assert!(harness.sent.lock().unwrap().is_empty());
}

#[test]
fn the_tray_s_lock_with_a_dead_page_locks_the_core_itself() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 3);
    // The page went away: its subscribers are gone with it.
    events(&harness.app).clear_subscribers();
    desktop::perform(harness.app.handle(), TrayAction::Lock);
    wait_until("the core is locked", || {
        harness.state.state_info() == atomik_kb_desktop_lib::CoreStateInfo::Locked
    });
}

#[test]
fn the_state_of_the_shortcut_is_sent_to_every_page_that_subscribes() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    events(&harness.app).set_shortcut_state(ShortcutState::Taken);

    subscribe(&webview, 2);
    events(&harness.app).set_shortcut_state(ShortcutState::Registered);

    assert_eq!(
        *harness.sent.lock().unwrap(),
        vec![
            (
                2,
                json!({ "type": "shortcut", "data": { "state": "taken" } })
            ),
            (
                2,
                json!({ "type": "shortcut", "data": { "state": "registered" } })
            ),
        ]
    );
}

// --- shell_labels_set ---

#[test]
fn the_labels_the_ui_sends_are_kept() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let store = Arc::clone(harness.app.state::<Arc<LabelStore>>().inner());
    assert_eq!(store.get(), Labels::default());

    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "open": "Ouvrir", "newPage": "Nouvelle page", "lock": "Verrouiller", "quit": "Quitter" } }),
    )
    .unwrap();
    let labels = store.get();
    assert_eq!(
        (
            &labels.open[..],
            &labels.new_page[..],
            &labels.lock[..],
            &labels.quit[..]
        ),
        ("Ouvrir", "Nouvelle page", "Verrouiller", "Quitter")
    );

    // A message names only what changes.
    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "quit": "Salir" } }),
    )
    .unwrap();
    assert_eq!(store.get().quit, "Salir");
    assert_eq!(store.get().open, "Ouvrir");
}

#[test]
fn the_words_of_the_dialogs_and_the_file_types_come_through_the_same_command() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let store = Arc::clone(harness.app.state::<Arc<LabelStore>>().inner());

    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": {
            "dialogAllow": "Autoriser",
            "dialogAiMessage": "Envoyer le texte \u{e0} {host} ?",
            "dialogMcpBoth": "{client} lit {read} et modifie {write} ?",
            "fileFilterZip": "Archive zip",
        } }),
    )
    .unwrap();
    let labels = store.get();
    assert_eq!(labels.dialog_allow, "Autoriser");
    assert_eq!(labels.dialog_ai_message, "Envoyer le texte \u{e0} {host} ?");
    assert_eq!(
        labels.dialog_mcp_both,
        "{client} lit {read} et modifie {write} ?"
    );
    assert_eq!(labels.file_filter_zip, "Archive zip");
    // What was not named keeps its English.
    assert_eq!(labels.dialog_cancel, "Cancel");
    assert_eq!(labels.dialog_mcp_read, "Allow {client} to read {spaces}?");

    // A consent text that hides what is agreed to, or fills a placeholder the shell does not, is refused by
    // field, and nothing in the message is kept.
    for (labels, field) in [
        (
            json!({ "dialogAiMessage": "Envoyer le texte ?" }),
            "dialogAiMessage",
        ),
        (
            json!({ "dialogMcpRead": "{client} peut lire ?" }),
            "dialogMcpRead",
        ),
        (
            json!({ "dialogAiMessage": "Vers {host} {secret}" }),
            "dialogAiMessage",
        ),
        (
            json!({ "dialogCancel": "Annuler", "dialogAiMessage": "x".repeat(401) }),
            "dialogAiMessage",
        ),
    ] {
        let error = call(&webview, "shell_labels_set", json!({ "labels": labels })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{labels}");
        assert_eq!(error["details"]["field"], field, "{labels}");
    }
    assert_eq!(store.get(), labels);
}

#[test]
fn a_label_that_is_not_valid_is_refused_by_field_and_nothing_changes() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let store = Arc::clone(harness.app.state::<Arc<LabelStore>>().inner());

    for (labels, field) in [
        (json!({ "open": "" }), "open"),
        (json!({ "newPage": "   " }), "newPage"),
        (json!({ "lock": "a".repeat(65) }), "lock"),
        (json!({ "quit": "Qu\nit" }), "quit"),
        // The first valid one in the message does not survive the second's refusal.
        (json!({ "open": "Ouvrir", "quit": "\u{0}" }), "quit"),
    ] {
        let error = call(&webview, "shell_labels_set", json!({ "labels": labels })).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{labels}");
        assert_eq!(error["details"]["field"], field, "{labels}");
    }
    assert_eq!(store.get(), Labels::default());
}

#[test]
fn an_error_about_a_label_never_repeats_it() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let error = call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "open": format!("SECRETLABEL{}", "x".repeat(80)) } }),
    )
    .unwrap_err();
    assert!(!error.to_string().contains("SECRETLABEL"), "{error}");
}

#[test]
fn a_label_id_the_shell_does_not_know_and_a_label_that_is_not_text_are_refused_at_the_door() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    for labels in [
        json!({ "nope": "x" }),
        json!({ "open": 5 }),
        json!({ "new_page": "x" }),
    ] {
        let error = call(&webview, "shell_labels_set", json!({ "labels": labels })).unwrap_err();
        assert!(error.is_string(), "{labels}: {error}");
    }
    // And the argument itself is needed.
    assert!(call(&webview, "shell_labels_set", json!({})).is_err());
    let store = harness.app.state::<Arc<LabelStore>>();
    assert_eq!(store.get(), Labels::default());
}

// --- the tray on the mock runtime ---

/// Builds the tray on the mock runtime, or `None` when this session has no notification area to build it in.
fn tray_or_skip(harness: &Harness) -> Option<()> {
    if desktop::install_tray(harness.app.handle()) {
        Some(())
    } else {
        eprintln!("skipped: no notification area in this session");
        None
    }
}

#[test]
#[cfg_attr(
    target_os = "linux",
    ignore = "builds a GTK menu, which a headless runner has not initialised"
)]
fn the_tray_takes_its_labels_from_the_store_and_follows_the_ui() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    // The UI had already sent its words when the tray was built.
    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "open": "Ouvrir" } }),
    )
    .unwrap();
    let Some(()) = tray_or_skip(&harness) else {
        return;
    };
    let tray = harness
        .app
        .state::<atomik_kb_desktop_lib::tray::Tray<MockRuntime>>();
    assert_eq!(
        tray.texts().unwrap(),
        ["Ouvrir", "New page", "Lock", "Quit"].map(str::to_owned)
    );
    // The tray exists now: the close button may hide the window.
    assert!(prefs(&harness.app).tray_ready());

    // And a later message changes the menu.
    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "newPage": "Nouvelle page", "quit": "Quitter" } }),
    )
    .unwrap();
    assert_eq!(
        tray.texts().unwrap(),
        ["Ouvrir", "Nouvelle page", "Lock", "Quitter"].map(str::to_owned)
    );
}

// --- following the settings ---

/// Registers what it is told to and records it; refuses the accelerators in `taken`.
#[derive(Clone, Default)]
struct Hotkeys {
    registered: Arc<Mutex<Vec<String>>>,
    taken: Arc<Mutex<Vec<String>>>,
    registrations: Arc<AtomicUsize>,
}

impl Registrar for Hotkeys {
    fn register(&mut self, accelerator: &Accelerator) -> bool {
        self.registrations.fetch_add(1, Ordering::SeqCst);
        let text = accelerator.normalised();
        if self.taken.lock().unwrap().contains(&text) {
            return false;
        }
        self.registered.lock().unwrap().push(text);
        true
    }

    fn unregister(&mut self, accelerator: &Accelerator) {
        let text = accelerator.normalised();
        self.registered
            .lock()
            .unwrap()
            .retain(|other| *other != text);
    }
}

struct Worker {
    worker: SettingsWorker<Hotkeys>,
    hotkeys: Hotkeys,
    prefs: Arc<DesktopPrefs>,
    events: Arc<ShellEvents>,
    state: Arc<AppState<Fake>>,
    world: Shared,
}

fn worker() -> Worker {
    let (state, world) = Fake::ready();
    let hotkeys = Hotkeys::default();
    let prefs = Arc::new(DesktopPrefs::new());
    let events = Arc::new(ShellEvents::new());
    let host: Arc<dyn CoreHost> = Arc::clone(&state) as Arc<dyn CoreHost>;
    let worker = SettingsWorker::new(
        Arc::downgrade(&host),
        Arc::clone(&prefs),
        Arc::clone(&events),
        Controller::new(hotkeys.clone()),
    );
    // `host` is dropped here on purpose: the worker holds the state weakly, and `state` keeps it alive.
    Worker {
        worker,
        hotkeys,
        prefs,
        events,
        state,
        world,
    }
}

fn set(world: &Shared, key: &str, value: Value) {
    world.lock().unwrap().settings.insert(key.to_owned(), value);
}

#[test]
fn without_any_setting_the_defaults_apply() {
    let mut w = worker();
    assert!(w.worker.refresh());
    assert!(w.prefs.close_to_tray());
    assert_eq!(
        *w.hotkeys.registered.lock().unwrap(),
        vec![DEFAULT_CAPTURE_SHORTCUT.to_owned()]
    );
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Registered));
}

#[test]
fn the_settings_that_are_stored_are_applied() {
    let mut w = worker();
    set(&w.world, CLOSE_TO_TRAY_SETTING, json!(false));
    set(&w.world, CAPTURE_SHORTCUT_SETTING, json!("ctrl+shift+k"));
    assert!(w.worker.refresh());
    assert!(!w.prefs.close_to_tray());
    assert_eq!(
        *w.hotkeys.registered.lock().unwrap(),
        vec!["Ctrl+Shift+K".to_owned()]
    );
}

#[test]
fn a_changed_shortcut_replaces_the_old_one_and_an_empty_one_removes_it() {
    let mut w = worker();
    w.worker.refresh();
    set(&w.world, CAPTURE_SHORTCUT_SETTING, json!("Alt+Space"));
    w.worker.refresh();
    assert_eq!(
        *w.hotkeys.registered.lock().unwrap(),
        vec!["Alt+Space".to_owned()]
    );
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Registered));

    set(&w.world, CAPTURE_SHORTCUT_SETTING, json!(""));
    w.worker.refresh();
    assert!(w.hotkeys.registered.lock().unwrap().is_empty());
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Disabled));
}

#[test]
fn an_invalid_shortcut_registers_nothing_and_says_so() {
    let mut w = worker();
    set(&w.world, CAPTURE_SHORTCUT_SETTING, json!("Shift+N"));
    w.worker.refresh();
    assert!(w.hotkeys.registered.lock().unwrap().is_empty());
    assert_eq!(w.hotkeys.registrations.load(Ordering::SeqCst), 0);
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Invalid));
}

#[test]
fn a_shortcut_another_program_owns_is_reported_and_the_app_keeps_running() {
    let mut w = worker();
    w.hotkeys
        .taken
        .lock()
        .unwrap()
        .push("Ctrl+Alt+N".to_owned());
    assert!(w.worker.refresh());
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Taken));
    // The other setting is still followed.
    set(&w.world, CLOSE_TO_TRAY_SETTING, json!(false));
    assert!(w.worker.refresh());
    assert!(!w.prefs.close_to_tray());
    // And the shell tries again when it reads the settings again (the other program may have let go).
    w.hotkeys.taken.lock().unwrap().clear();
    w.worker.refresh();
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Registered));
}

#[test]
fn a_locked_core_changes_nothing() {
    let mut w = worker();
    set(&w.world, CLOSE_TO_TRAY_SETTING, json!(false));
    w.worker.refresh();
    assert!(!w.prefs.close_to_tray());
    let registered = w.hotkeys.registered.lock().unwrap().clone();

    w.state.lock().unwrap();
    // While locked the settings cannot be read: what was read last stays in force.
    set(&w.world, CLOSE_TO_TRAY_SETTING, json!(true));
    assert!(!w.worker.refresh());
    assert!(!w.prefs.close_to_tray());
    assert_eq!(*w.hotkeys.registered.lock().unwrap(), registered);
    assert_eq!(w.events.shortcut_state(), Some(ShortcutState::Registered));
}

#[test]
fn a_core_that_is_still_opening_changes_nothing() {
    let state = Arc::new(AppState::<Fake>::new());
    let hotkeys = Hotkeys::default();
    let prefs = Arc::new(DesktopPrefs::new());
    let events = Arc::new(ShellEvents::new());
    let host: Arc<dyn CoreHost> = Arc::clone(&state) as Arc<dyn CoreHost>;
    let mut worker = SettingsWorker::new(
        Arc::downgrade(&host),
        Arc::clone(&prefs),
        Arc::clone(&events),
        Controller::new(hotkeys.clone()),
    );
    assert!(!worker.refresh());
    // The defaults stay: close to the tray on, and no shortcut registered until the settings are known.
    assert!(prefs.close_to_tray());
    assert!(hotkeys.registered.lock().unwrap().is_empty());
    assert_eq!(events.shortcut_state(), None);
}

#[test]
fn a_worker_whose_state_is_gone_does_nothing() {
    let gone: std::sync::Weak<dyn CoreHost> = std::sync::Weak::<AppState<Fake>>::new();
    let prefs = Arc::new(DesktopPrefs::new());
    let events = Arc::new(ShellEvents::new());
    let mut worker = SettingsWorker::new(
        gone,
        Arc::clone(&prefs),
        Arc::clone(&events),
        Controller::new(Hotkeys::default()),
    );
    assert!(!worker.refresh());
    assert_eq!(events.shortcut_state(), None);
}

// --- the listener, the channel and the worker thread ---

/// Waits for `condition` for up to five seconds.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[cfg_attr(
    target_os = "linux",
    ignore = "builds a GTK menu, which a headless runner has not initialised"
)]
fn a_setting_written_through_the_webview_reaches_the_worker_through_the_core_s_event() {
    // The whole chain on the mock runtime: `dispatch` of a `SettingsSet` -> the core's `SettingChanged` ->
    // the listener `desktop::start` registered -> the worker thread -> `DesktopPrefs` and the shortcut's
    // state. (The tray it builds is the real one, or none in a session without a notification area; the
    // global-shortcut plugin is not loaded on the mock runtime, so the shortcut is `taken` here.)
    let harness = harness();
    let webview = main_webview(&harness.app);
    let shared: SharedState = Arc::clone(&harness.state) as SharedState;
    let starter = desktop::start(harness.app.handle(), &shared);
    let prefs = prefs(&harness.app);
    let shortcuts = events(&harness.app);

    // The core became ready: the starter signals the worker, which reads the (absent) settings.
    assert!(starter().is_none(), "there is nothing to stop");
    wait_until("the first read", || shortcuts.shortcut_state().is_some());
    assert!(prefs.close_to_tray());
    assert_eq!(shortcuts.shortcut_state(), Some(ShortcutState::Taken));

    // The Settings screen turns the switch off, through the webview's `dispatch`.
    call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": { "key": CLOSE_TO_TRAY_SETTING, "value": false } } }),
    )
    .unwrap();
    wait_until("close to tray is off", || !prefs.close_to_tray());

    // And back on.
    call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": { "key": CLOSE_TO_TRAY_SETTING, "value": true } } }),
    )
    .unwrap();
    wait_until("close to tray is on", || prefs.close_to_tray());

    // A shortcut that is not valid is reported (the state is what the page is told).
    call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": { "key": CAPTURE_SHORTCUT_SETTING, "value": "Shift+N" } } }),
    )
    .unwrap();
    wait_until("the shortcut is invalid", || {
        shortcuts.shortcut_state() == Some(ShortcutState::Invalid)
    });
    assert_eq!(
        harness
            .world
            .lock()
            .unwrap()
            .settings
            .get(CAPTURE_SHORTCUT_SETTING),
        Some(&json!("Shift+N"))
    );
}

#[test]
#[cfg_attr(
    target_os = "linux",
    ignore = "builds a GTK menu, which a headless runner has not initialised"
)]
fn other_settings_do_not_wake_the_worker() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let shared: SharedState = Arc::clone(&harness.state) as SharedState;
    let starter = desktop::start(harness.app.handle(), &shared);
    let shortcuts = events(&harness.app);
    starter();
    wait_until("the first read", || shortcuts.shortcut_state().is_some());
    let reads_before = reads_of(&harness.world);

    call(
        &webview,
        "dispatch",
        json!({ "request": { "type": "SettingsSet", "data": { "key": "ui.theme", "value": "dark" } } }),
    )
    .unwrap();
    // Give a wrongly woken worker time to read.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(reads_of(&harness.world), reads_before);
}

fn reads_of(world: &Shared) -> usize {
    world
        .lock()
        .unwrap()
        .log
        .iter()
        .filter(|kind| kind.as_str() == "SettingsGet")
        .count()
}

// --- what the webview cannot do ---

#[test]
fn the_webview_cannot_call_the_global_shortcut_plugin_directly() {
    // The plugin is loaded by the real app only; here it is loaded for this test to show that, loaded, none
    // of its commands is allowed to the webview (the capability grants no `global-shortcut:*` permission).
    let (state, _) = Fake::ready();
    let shared: SharedState = state as SharedState;
    let app = configure(mock_builder(), shared).build(context()).unwrap();
    // Loaded the way `setup` loads it, into the running app.
    assert!(desktop::load_global_shortcut(app.handle()));
    assert!(
        app.try_state::<tauri_plugin_global_shortcut::GlobalShortcut<MockRuntime>>()
            .is_some()
    );
    let webview = main_webview(&app);
    for (command, args) in [
        (
            "plugin:global-shortcut|register",
            json!({ "shortcuts": ["Ctrl+Alt+N"], "handler": channel(1) }),
        ),
        (
            "plugin:global-shortcut|unregister",
            json!({ "shortcuts": ["Ctrl+Alt+N"] }),
        ),
        ("plugin:global-shortcut|unregister_all", json!({})),
        (
            "plugin:global-shortcut|is_registered",
            json!({ "shortcut": "Ctrl+Alt+N" }),
        ),
    ] {
        let error = call(&webview, command, args).unwrap_err();
        let message = error.as_str().unwrap_or_default();
        assert!(message.contains("not allowed"), "{command}: {error}");
    }
}

#[test]
fn the_capability_grants_the_webview_nothing_for_the_tray_or_the_shortcut() {
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    assert_eq!(capability["permissions"], json!(["core:default"]));
    let description = capability["description"].as_str().unwrap();
    for command in ["shell_events_subscribe", "shell_labels_set"] {
        assert!(description.contains(command), "{command}");
    }
    assert!(description.contains("tray"));
    assert!(description.contains("global-shortcut"));
}
