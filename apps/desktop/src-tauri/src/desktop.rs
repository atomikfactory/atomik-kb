//! The desktop shell's tray and quick capture (ADR-0040), wired together: the actions the tray menu and the
//! global shortcut perform, the labels the UI sends, and the worker that keeps the shell in step with the two
//! device settings.
//!
//! ```text
//!   tray menu / left click ──┐                       ┌── window::DesktopPrefs  (close to tray)
//!   global shortcut ─────────┼─► perform / capture   │
//!                            │        │              └── shortcut::Controller  (registers the accelerator)
//!   core's SettingChanged ───┴─► SettingsWorker ──────────► ShellEvents  (`shortcut` state to the UI)
//! ```
//!
//! **Actions.** *Open* shows, unminimises and focuses the main window. *New page* and the global shortcut do
//! that and tell the UI to `capture` (`shell_events`): the UI makes a page at the top level of the space it is
//! in, with its title ready to type, and holds the request until the workspace is open when the app is locked
//! or still opening. *Lock* is the UI's own lock action, so that the editor's unsaved text is sent before the
//! core drops its keys: the shell asks the UI (`lock` message) and, only when no page is there to ask, locks the
//! core itself. *Quit* is `window::quit`.
//!
//! **How the shell learns of the settings.** The shell cannot read a device setting before the core is open,
//! and the Settings screen writes them through the webview's `dispatch`. The shell registers a Rust listener on
//! the state's events (`CoreHost::add_listener`, which hears every event the core emits, whoever made the
//! request): a `SettingChanged` for one of the two keys, and the core becoming ready (at launch and after every
//! unlock, through a background starter), signal a worker thread. The worker reads both settings through the
//! core (`SettingsGet`, like any caller) and applies them: `app.tray.close_to_tray` into [`DesktopPrefs`],
//! `app.shortcut.capture` into the registered shortcut, and tells the UI the shortcut's state
//! ([`ShellEvents::set_shortcut_state`]) so Settings can say that another app holds it. The listener only sends
//! on a channel (it runs under the core's lock and must not call into the core); a burst of signals is one
//! read. While the core is locked or opening the read fails and nothing changes: **the values of the last
//! read stay in force, and before the first read the defaults do** (close to the tray on, and no shortcut
//! registered, because the stored accelerator is unknown until the first unlock).
//!
//! **The menu bar** (`menu_bar`) reuses these actions: its New page is [`capture`], its Lock is [`lock`] and its Quit
//! is [`window::quit`], so the tray, the shortcut and the menu bar make the same messages and the same exit.
//!
//! **What is not covered by the mock runtime.** The tray icon and the global shortcut need an operating
//! system's notification area and hotkey table; the tests run the logic around them (`accelerator`, `shortcut`'s
//! controller with a fake registrar, `window`, the worker over the scripted core, the actions on the mock
//! runtime) and the real-app pass checks the rest.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Weak};

use kb_types::{ApiError, Event, Request, Response};
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::ShortcutEvent;

use crate::desktop_settings::{
    self, CAPTURE_SHORTCUT_SETTING, CLOSE_TO_TRAY_SETTING, CaptureShortcut,
};
use crate::menu_bar;
use crate::shell_events::ShellEvents;
use crate::shell_labels::{LabelStore, ShellLabels};
use crate::shortcut::{Controller, PluginRegistrar, Registrar};
use crate::state::{BackgroundStarter, CoreHost};
use crate::tray::{Tray, TrayAction};
use crate::window::{self, DesktopPrefs};
use crate::{SharedState, shortcut};

/// Whether a setting key is one the shell reads here.
#[must_use]
pub fn is_desktop_setting(key: &str) -> bool {
    key == CLOSE_TO_TRAY_SETTING || key == CAPTURE_SHORTCUT_SETTING
}

/// What the tray menu's items and the global shortcut do (see the module's text).
pub fn perform<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    match action {
        TrayAction::Open => {
            window::show_main_window(app);
        }
        TrayAction::NewPage => capture(app),
        TrayAction::Lock => lock(app),
        TrayAction::Quit => window::quit(app),
    }
}

/// Quick capture: brings the main window to the front and tells the UI to make a page. With no page to tell
/// (the app is starting, or its page is reloading) the request is held for the next subscriber.
pub fn capture<R: Runtime>(app: &AppHandle<R>) {
    window::show_main_window(app);
    if let Some(events) = app.try_state::<Arc<ShellEvents>>() {
        events.capture();
    }
}

/// The tray's "Lock": the UI's lock action when a page can be asked, else the core's lock called directly.
pub fn lock<R: Runtime>(app: &AppHandle<R>) {
    if app
        .try_state::<Arc<ShellEvents>>()
        .is_some_and(|events| events.lock())
    {
        return;
    }
    let Some(state) = app.try_state::<SharedState>() else {
        return;
    };
    let state = Arc::clone(&state);
    // The core call may take the single-writer lock: not on the thread of the tray's click.
    tauri::async_runtime::spawn_blocking(move || match state.lock() {
        Ok(()) => tracing::info!("the app was locked from the tray"),
        Err(error) => tracing::debug!(code = ?error.code, "the tray could not lock the app"),
    });
}

/// What the global-shortcut plugin does with an event: a press is a capture, a release is nothing.
pub fn on_shortcut_event<R: Runtime>(app: &AppHandle<R>, event: &ShortcutEvent) {
    if shortcut::is_press(event) {
        capture(app);
    }
}

/// The `tauri-plugin-global-shortcut` plugin with the handler above. The webview is granted none of its
/// permissions; only the worker registers a shortcut.
pub fn global_shortcut_plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| on_shortcut_event(app, &event))
        .build()
}

/// Loads [`global_shortcut_plugin`] into the running app (from `setup`, on the main thread, where the plugin
/// creates its hotkey manager). It is loaded here, not on the builder, because a plugin that fails to set up
/// (Linux with no X display to register a hotkey on) would end the app before it starts: this way the failure is
/// logged and the app runs without the shortcut, which Settings then reports as not registered. `true` when it
/// loaded.
pub fn load_global_shortcut<R: Runtime>(app: &AppHandle<R>) -> bool {
    match app.plugin(global_shortcut_plugin()) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "the global shortcut plugin could not be loaded: the quick capture has no shortcut");
            false
        }
    }
}

/// `shell_labels_set`: stores the labels the UI sent (all or none, see `shell_labels`) and puts them on the tray
/// menu and on the menu bar when there are some.
///
/// # Errors
///
/// `InvalidInput` for the field of the first label that is not valid.
pub fn set_labels<R: Runtime>(app: &AppHandle<R>, update: &ShellLabels) -> Result<(), ApiError> {
    let Some(store) = app.try_state::<Arc<LabelStore>>() else {
        return Err(ApiError::internal("the shell has no labels to change"));
    };
    let labels = store.update(update)?;
    if let Some(tray) = app.try_state::<Tray<R>>()
        && tray.set_labels(&labels).is_err()
    {
        // Never the text: a label is the person's language.
        tracing::warn!("a tray label could not be changed");
    }
    menu_bar::apply_labels(app, &labels);
    Ok(())
}

/// Builds the tray icon with the labels of the moment and keeps it in the managed state; the close button hides
/// the window only once this has worked. A tray that cannot be built (no status-notifier host on Linux) is
/// logged and the app runs without one.
pub fn install_tray<R: Runtime>(app: &AppHandle<R>) -> bool {
    let labels = app
        .try_state::<Arc<LabelStore>>()
        .map(|store| store.get())
        .unwrap_or_default();
    match Tray::build(app, &labels) {
        Ok(tray) => {
            app.manage(tray);
            if let Some(prefs) = app.try_state::<Arc<DesktopPrefs>>() {
                prefs.set_tray_ready(true);
            }
            tracing::info!("the tray icon is ready");
            true
        }
        Err(error) => {
            tracing::warn!(%error, "the tray icon could not be built: closing the window exits");
            false
        }
    }
}

/// The two settings as the shell applies them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopSettings {
    /// `app.tray.close_to_tray`.
    pub close_to_tray: bool,
    /// `app.shortcut.capture`.
    pub capture: CaptureShortcut,
}

/// Reads both settings through the core.
///
/// # Errors
///
/// Why the core cannot answer (locked, still opening, failed), or `Internal` for an answer of another kind.
pub fn read_settings(host: &dyn CoreHost) -> Result<DesktopSettings, ApiError> {
    let read = |key: &str| -> Result<Option<kb_types::Json>, ApiError> {
        match host.dispatch(Request::SettingsGet {
            key: key.to_owned(),
        })? {
            Response::Setting(value) => Ok(value),
            _ => Err(ApiError::internal(
                "the core answered a setting with another kind",
            )),
        }
    };
    Ok(DesktopSettings {
        close_to_tray: desktop_settings::close_to_tray(read(CLOSE_TO_TRAY_SETTING)?.as_ref()),
        capture: desktop_settings::capture_shortcut(read(CAPTURE_SHORTCUT_SETTING)?.as_ref()),
    })
}

/// Reads the settings and applies them: [`DesktopPrefs`] for the close button, the registered shortcut, and the
/// shortcut's state to the UI.
pub struct SettingsWorker<G: Registrar> {
    host: Weak<dyn CoreHost>,
    prefs: Arc<DesktopPrefs>,
    events: Arc<ShellEvents>,
    shortcuts: Controller<G>,
}

impl<G: Registrar> SettingsWorker<G> {
    /// A worker that reads through `host` (held weakly: it ends with the app) and applies into the rest.
    pub fn new(
        host: Weak<dyn CoreHost>,
        prefs: Arc<DesktopPrefs>,
        events: Arc<ShellEvents>,
        shortcuts: Controller<G>,
    ) -> Self {
        Self {
            host,
            prefs,
            events,
            shortcuts,
        }
    }

    /// One read and apply. `true` when the settings were read; `false` when the core could not answer (locked,
    /// still opening, gone), which changes nothing.
    pub fn refresh(&mut self) -> bool {
        let Some(host) = self.host.upgrade() else {
            return false;
        };
        let settings = match read_settings(&*host) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::debug!(code = ?error.code, "the desktop settings cannot be read yet");
                return false;
            }
        };
        self.prefs.set_close_to_tray(settings.close_to_tray);
        let state = self.shortcuts.apply(&settings.capture);
        self.events.set_shortcut_state(state);
        // Both are states, never the accelerator.
        tracing::info!(
            close_to_tray = settings.close_to_tray,
            shortcut = ?state,
            "the desktop settings were applied"
        );
        true
    }
}

/// Runs `worker.refresh()` for every signal, until every sender is gone. A burst of signals is one refresh.
fn run_worker<G: Registrar>(mut worker: SettingsWorker<G>, signals: &Receiver<()>) {
    while signals.recv().is_ok() {
        while signals.try_recv().is_ok() {}
        worker.refresh();
    }
}

/// Starts the shell's side of the tray and quick capture: builds the tray, and starts the worker that follows
/// the two settings (see the module's text). Returns the background starter that tells the worker the core is
/// ready; add it to the others with `all_of`.
pub fn start<R: Runtime>(app: &AppHandle<R>, state: &SharedState) -> BackgroundStarter {
    install_tray(app);

    let (signal, signals) = mpsc::channel::<()>();
    let from_events = signal.clone();
    // Never removed: the listener lives as long as the state. It runs under the core's lock, so it only signals.
    let _ = state.add_listener(Arc::new(move |event: &Event| {
        if let Event::SettingChanged { key } = event
            && is_desktop_setting(key)
        {
            let _ = from_events.send(());
        }
    }));

    match (
        app.try_state::<Arc<DesktopPrefs>>(),
        app.try_state::<Arc<ShellEvents>>(),
    ) {
        (Some(prefs), Some(events)) => {
            let worker = SettingsWorker::new(
                Arc::downgrade(state),
                Arc::clone(&prefs),
                Arc::clone(&events),
                Controller::new(PluginRegistrar::new(app.clone())),
            );
            let spawned = std::thread::Builder::new()
                .name("desktop-settings".to_owned())
                .spawn(move || run_worker(worker, &signals));
            if let Err(error) = spawned {
                tracing::warn!(%error, "the desktop settings worker could not be started");
            }
        }
        _ => tracing::warn!("the desktop shell's state is missing: its settings are not followed"),
    }

    // The core became ready (at launch, after an unlock): read the settings again. Nothing to stop.
    Box::new(move || {
        let _ = signal.send(());
        None
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_reads_exactly_two_settings() {
        assert!(is_desktop_setting("app.tray.close_to_tray"));
        assert!(is_desktop_setting("app.shortcut.capture"));
        for other in [
            "app.tray",
            "app.shortcut.capture ",
            "ui.theme",
            "app.backup.folder",
            "",
        ] {
            assert!(!is_desktop_setting(other), "{other:?}");
        }
    }
}
