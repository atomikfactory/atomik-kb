//! The native menu bar (ADR-0040 item 12) through Tauri's real IPC layer, on the mock runtime: that the menu builds
//! with the UI's labels and follows them, that `menu_bar_show` and `menu_bar_hide` change the bar's state and tell
//! the page, that a choice hides the bar and runs the item (the shell's own, or a `menu` message for the UI), and
//! that nothing breaks without a menu bar.
//!
//! What the mock runtime cannot do: put a menu bar on a screen. Its windows have no native handle, so showing and
//! hiding change the shell's state and the page's `menuBar` message but not a window, and nothing can be clicked
//! or typed at. Whether the bar really appears on Alt, shifts the page's content and hides again, whether the
//! keys reach the native menu, and what each native item does, are the real-app pass (`docs/architecture/overview.md`).
//! The menu itself (the muda items) is real on Windows even here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

mod common;

use std::sync::{Arc, Mutex};

use atomik_kb_desktop_lib::menu_bar::{
    self, Capabilities, Item, MenuBar, describe, item_of, labels_of,
};
use atomik_kb_desktop_lib::shell_labels::Labels;
use atomik_kb_desktop_lib::{
    LabelStore, MAIN_WINDOW, SharedState, ShellEvents, configure, context, window,
};
use common::Fake;
use serde_json::{Value, json};
use tauri::http::HeaderMap;
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::menu::{MenuEvent, MenuId, MenuItemKind};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// What the shell sent to channels, as `(callback id, JSON message)`.
type Sent = Arc<Mutex<Vec<(u32, Value)>>>;

struct Harness {
    app: App<MockRuntime>,
    sent: Sent,
}

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
    let (state, _world) = Fake::ready();
    let shared: SharedState = state as SharedState;
    let app = configure(builder, shared).build(context()).unwrap();
    Harness { app, sent }
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

fn subscribe(webview: &WebviewWindow<MockRuntime>, id: u32) {
    call(
        webview,
        "shell_events_subscribe",
        json!({ "onEvent": format!("__CHANNEL__:{id}") }),
    )
    .unwrap();
}

/// What the page was sent so far, without the channel ids.
fn messages(harness: &Harness) -> Vec<Value> {
    harness
        .sent
        .lock()
        .unwrap()
        .iter()
        .map(|(_, message)| message.clone())
        .collect()
}

fn menu_bar_of(app: &App<MockRuntime>) -> tauri::State<'_, MenuBar<MockRuntime>> {
    app.state::<MenuBar<MockRuntime>>()
}

/// Installs the menu bar on the main window the way `setup` does. On Windows (the built platform) this must work; a
/// session that has no windowing system to build a menu in (a Linux run without a display) skips instead.
fn installed_or_skip(harness: &Harness) -> Option<()> {
    if menu_bar::install(harness.app.handle()) {
        Some(())
    } else {
        #[cfg(windows)]
        panic!("the menu bar could not be built on Windows");
        #[cfg(not(windows))]
        {
            eprintln!("skipped: no menu bar could be built in this session");
            None
        }
    }
}

fn choose(harness: &Harness, id: &str) {
    menu_bar::on_menu_event(
        harness.app.handle(),
        &MenuEvent {
            id: MenuId::new(id),
        },
    );
}

// --- the menu ---

#[test]
fn the_menu_is_built_with_the_english_labels_and_hidden() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    let bar = menu_bar_of(&harness.app);
    // On the window, and not shown.
    assert!(webview.menu().is_some());
    assert!(!bar.is_visible());

    let expected = labels_of(&describe(&Labels::default(), Capabilities::current()));
    assert_eq!(bar.labels().unwrap(), expected);

    // Five menus, in order, with the entries the description lists (a separator is an item of its own).
    let menus = bar.menu().items().unwrap();
    let ids: Vec<String> = menus
        .iter()
        .map(|kind| kind.id().as_ref().to_owned())
        .collect();
    assert_eq!(
        ids,
        vec![
            "menu-file",
            "menu-edit",
            "menu-view",
            "menu-window",
            "menu-help"
        ]
    );
    let described = describe(&Labels::default(), Capabilities::current());
    for (kind, section) in menus.iter().zip(&described) {
        let MenuItemKind::Submenu(submenu) = kind else {
            panic!("{} is not a menu", section.id);
        };
        assert_eq!(submenu.items().unwrap().len(), section.entries.len());
    }
    // Every item of ours is in it, under its id, with the label of the description.
    let find = |id: &str| {
        menus.iter().find_map(|kind| match kind {
            MenuItemKind::Submenu(submenu) => submenu.get(id),
            _ => None,
        })
    };
    for item in Item::ALL {
        let found = find(item.id());
        if cfg!(windows) && matches!(item, Item::Undo | Item::Redo) {
            // Native on Windows: muda gives those items ids of its own.
            assert!(found.is_none(), "{item:?}");
            continue;
        }
        let Some(MenuItemKind::MenuItem(menu_item)) = found else {
            panic!("{item:?} is not in the menu");
        };
        assert!(menu_item.is_enabled().unwrap(), "{item:?}");
    }
}

#[test]
fn the_labels_the_ui_sent_before_the_menu_was_built_are_in_it() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "menuFile": "&Datei", "menuPrint": "&Drucken\u{2026}" } }),
    )
    .unwrap();
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    let labels = menu_bar_of(&harness.app).labels().unwrap();
    assert_eq!(labels[0], "&Datei");
    assert!(labels.contains(&"&Drucken\u{2026}".to_owned()));
    assert!(labels.contains(&"&Quit".to_owned()));
}

#[test]
fn the_menu_follows_the_labels_the_ui_sends_in_place() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": {
            "menuFile": "&Fichier", "menuNewPage": "&Nouvelle page", "menuImport": "&Importer\u{2026}",
            "menuCopy": "&Copier", "menuAbout": "&\u{c0} propos"
        } }),
    )
    .unwrap();
    let bar = menu_bar_of(&harness.app);
    let labels = bar.labels().unwrap();
    assert_eq!(labels[0], "&Fichier");
    for word in [
        "&Nouvelle page",
        "&Importer\u{2026}",
        "&Copier",
        "&\u{c0} propos",
    ] {
        assert!(labels.contains(&word.to_owned()), "{word}");
    }
    // What was not named kept its English words, the hints included: the label alone is read back.
    assert!(labels.contains(&"&Export\u{2026}".to_owned()));
    assert!(labels.contains(&"&Print\u{2026}".to_owned()));
    // Same menu, same shape: no item was added or lost.
    let expected = labels_of(&describe(&Labels::default(), Capabilities::current())).len();
    assert_eq!(labels.len(), expected);

    // The tray's words are a separate message and do not touch the menu bar.
    call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "quit": "Quitter" } }),
    )
    .unwrap();
    assert_eq!(bar.labels().unwrap(), labels);
}

#[test]
fn a_label_that_is_refused_changes_nothing_on_the_menu() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    let before = menu_bar_of(&harness.app).labels().unwrap();
    let error = call(
        &webview,
        "shell_labels_set",
        json!({ "labels": { "menuFile": "&Datei", "menuHelp": "Ai\nde" } }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(error["details"]["field"], "menuHelp");
    assert_eq!(menu_bar_of(&harness.app).labels().unwrap(), before);
    assert_eq!(
        harness.app.state::<Arc<LabelStore>>().get(),
        Labels::default()
    );
}

#[test]
fn a_label_id_that_is_not_a_menu_label_is_refused() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    for labels in [
        json!({ "menuFiles": "x" }),
        json!({ "menu_file": "x" }),
        json!({ "menuFile": 3 }),
    ] {
        let error = call(&webview, "shell_labels_set", json!({ "labels": labels })).unwrap_err();
        assert!(error.is_string(), "{labels}: {error}");
    }
}

// --- showing and hiding ---

#[test]
fn a_lone_alt_shows_the_bar_once_and_the_page_is_told() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);
    let bar = menu_bar_of(&harness.app);
    assert!(!bar.is_visible());

    call(&webview, "menu_bar_show", json!({})).unwrap();
    assert!(bar.is_visible());
    // The second press is the operating system's (its menu mode): the shell leaves the bar on the window, and
    // says nothing more.
    call(&webview, "menu_bar_show", json!({})).unwrap();
    assert!(bar.is_visible());
    assert_eq!(
        messages(&harness),
        vec![json!({ "type": "menuBar", "data": { "visible": true } })]
    );

    // Hidden by something else, a lone Alt shows it again.
    call(&webview, "menu_bar_hide", json!({})).unwrap();
    call(&webview, "menu_bar_show", json!({})).unwrap();
    assert!(bar.is_visible());
    assert_eq!(
        messages(&harness),
        vec![
            json!({ "type": "menuBar", "data": { "visible": true } }),
            json!({ "type": "menuBar", "data": { "visible": false } }),
            json!({ "type": "menuBar", "data": { "visible": true } }),
        ]
    );
}

#[test]
fn hide_hides_a_shown_bar_once_and_says_nothing_when_it_is_already_hidden() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);

    call(&webview, "menu_bar_hide", json!({})).unwrap();
    assert!(harness.sent.lock().unwrap().is_empty());

    call(&webview, "menu_bar_show", json!({})).unwrap();
    call(&webview, "menu_bar_hide", json!({})).unwrap();
    call(&webview, "menu_bar_hide", json!({})).unwrap();
    assert!(!menu_bar_of(&harness.app).is_visible());
    assert_eq!(
        messages(&harness),
        vec![
            json!({ "type": "menuBar", "data": { "visible": true } }),
            json!({ "type": "menuBar", "data": { "visible": false } }),
        ]
    );
}

#[test]
fn losing_the_focus_hides_a_shown_bar() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);
    call(&webview, "menu_bar_show", json!({})).unwrap();

    // Gaining the focus changes nothing; losing it hides the bar.
    let main = tauri::Webview::window(webview.as_ref());
    window::on_window_event(&main, &WindowEvent::Focused(true));
    assert!(menu_bar_of(&harness.app).is_visible());
    window::on_window_event(&main, &WindowEvent::Focused(false));
    assert!(!menu_bar_of(&harness.app).is_visible());
    assert_eq!(
        messages(&harness).last().unwrap(),
        &json!({ "type": "menuBar", "data": { "visible": false } })
    );
}

#[test]
fn without_a_menu_bar_the_commands_do_nothing_and_succeed() {
    // macOS, or a bar that could not be built: the app runs as before.
    let harness = harness();
    let webview = main_webview(&harness.app);
    subscribe(&webview, 5);
    assert!(harness.app.try_state::<MenuBar<MockRuntime>>().is_none());
    call(&webview, "menu_bar_show", json!({})).unwrap();
    call(&webview, "menu_bar_hide", json!({})).unwrap();
    assert!(harness.sent.lock().unwrap().is_empty());
    assert!(!menu_bar::show(harness.app.handle()));
}

// --- a choice ---

#[test]
fn a_choice_hides_the_bar_and_then_asks_the_page_to_run_the_item() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);
    call(&webview, "menu_bar_show", json!({})).unwrap();

    choose(&harness, Item::Print.id());

    assert!(!menu_bar_of(&harness.app).is_visible());
    assert_eq!(
        messages(&harness),
        vec![
            json!({ "type": "menuBar", "data": { "visible": true } }),
            json!({ "type": "menuBar", "data": { "visible": false } }),
            json!({ "type": "menu", "data": { "id": "print" } }),
        ]
    );
}

#[test]
fn every_item_the_ui_runs_reaches_it_under_its_own_name() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);
    for (item, name) in [
        (Item::Import, "import"),
        (Item::Export, "export"),
        (Item::Print, "print"),
        (Item::Undo, "undo"),
        (Item::Redo, "redo"),
        (Item::Find, "find"),
        (Item::ToggleSidebar, "toggleSidebar"),
        (Item::Back, "back"),
        (Item::Forward, "forward"),
        (Item::About, "about"),
    ] {
        harness.sent.lock().unwrap().clear();
        choose(&harness, item.id());
        assert_eq!(
            messages(&harness),
            vec![json!({ "type": "menu", "data": { "id": name } })],
            "{item:?}"
        );
    }
}

#[test]
fn new_page_is_the_quick_capture_and_lock_is_the_uis_own_lock_action() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);

    choose(&harness, Item::NewPage.id());
    choose(&harness, Item::Lock.id());

    // The same messages the tray's items send: the page makes the page, and locks itself.
    assert_eq!(
        messages(&harness),
        vec![json!({ "type": "capture" }), json!({ "type": "lock" })]
    );
}

#[test]
fn a_menu_item_that_is_not_ours_does_nothing() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    subscribe(&webview, 5);
    call(&webview, "menu_bar_show", json!({})).unwrap();
    harness.sent.lock().unwrap().clear();

    // The tray's items, a predefined item's own id, and nonsense: the bar stays and nothing is sent.
    for id in [
        "tray-quit",
        "tray-lock",
        "menu-bar",
        "menu-file",
        "",
        "copy",
    ] {
        assert_eq!(item_of(id), None, "{id:?}");
        choose(&harness, id);
    }
    assert!(menu_bar_of(&harness.app).is_visible());
    assert!(harness.sent.lock().unwrap().is_empty());
}

#[test]
fn a_choice_nobody_can_hear_is_dropped_not_held() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    let Some(()) = installed_or_skip(&harness) else {
        return;
    };
    // The page is still loading: nobody has subscribed yet.
    choose(&harness, Item::Export.id());
    assert!(!harness.app.state::<Arc<ShellEvents>>().has_pending());
    subscribe(&webview, 5);
    assert!(harness.sent.lock().unwrap().is_empty());
}

// --- the webview's permissions ---

#[test]
fn the_webview_may_call_the_two_menu_commands_and_its_capability_gained_nothing() {
    let harness = harness();
    let webview = main_webview(&harness.app);
    // The app's own commands need no permission entry.
    call(&webview, "menu_bar_show", json!({})).unwrap();
    call(&webview, "menu_bar_hide", json!({})).unwrap();
    // The capability still grants `core:default` and nothing else: the menu bar is built from Rust.
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    assert_eq!(capability["permissions"], json!(["core:default"]));
    let description = capability["description"].as_str().unwrap();
    for command in ["menu_bar_show", "menu_bar_hide"] {
        assert!(description.contains(command), "{command}");
    }
}
