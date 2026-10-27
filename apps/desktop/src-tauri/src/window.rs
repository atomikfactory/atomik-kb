//! The main window's life in the tray (ADR-0040): showing it, what its close button does, and quitting.
//!
//! **Close to tray.** While the device setting `app.tray.close_to_tray` is true (the default) and the tray icon
//! exists, the close button hides the window instead of closing it, so the process keeps running and the
//! reminders (ADR-0024) keep firing; the tray icon's left click and its menu bring the window back (so does a
//! second launch, `deep_link::on_second_launch`). With the setting false, or with no tray icon to bring the
//! window back from (the tray failed to build, as on a Linux desktop with no status-notifier host), closing
//! exits as it always did. [`close_decision`] is the pure rule; [`DesktopPrefs`] holds its three inputs, which
//! the shell's settings worker (`desktop`) keeps in step with the setting.
//!
//! **The menu bar.** A window that loses focus hides the native menu bar (`menu_bar`), which only a lone Alt
//! shows.
//!
//! **Quit.** The tray's "Quit" ends the process through the normal path: it only says that the app is quitting
//! (so that the close request is not turned into a hide) and closes the main window, which makes the same close
//! request the close button does, destroys the window and ends the run when the last window is gone. There is
//! no other exit path to flush: the core saves as it goes, the editor sends its unsaved text when the page
//! unloads, and nothing in the shell waits at exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

use crate::MAIN_WINDOW;

/// What the close button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// Hide the window; the app keeps running in the tray.
    Hide,
    /// Close the window as usual (the app exits with its last window).
    Close,
}

/// The rule: hide only when the person wants it, a tray icon exists to bring the window back, and the app is not
/// being quit.
#[must_use]
pub fn close_decision(close_to_tray: bool, tray_ready: bool, quitting: bool) -> CloseDecision {
    if close_to_tray && tray_ready && !quitting {
        CloseDecision::Hide
    } else {
        CloseDecision::Close
    }
}

/// The inputs of [`close_decision`], shared between the window's event handler and the shell's other threads.
/// Managed by Tauri inside an `Arc`.
#[derive(Debug)]
pub struct DesktopPrefs {
    close_to_tray: AtomicBool,
    tray_ready: AtomicBool,
    quitting: AtomicBool,
}

impl Default for DesktopPrefs {
    /// The setting's default (hide to the tray), no tray yet, not quitting.
    fn default() -> Self {
        Self {
            close_to_tray: AtomicBool::new(true),
            tray_ready: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
        }
    }
}

impl DesktopPrefs {
    /// The defaults: close to tray on, no tray icon yet, not quitting.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Follows the device setting `app.tray.close_to_tray`.
    pub fn set_close_to_tray(&self, on: bool) {
        self.close_to_tray.store(on, Ordering::SeqCst);
    }

    /// Whether closing the window hides it, as far as the setting goes.
    #[must_use]
    pub fn close_to_tray(&self) -> bool {
        self.close_to_tray.load(Ordering::SeqCst)
    }

    /// The tray icon exists (or no longer does).
    pub fn set_tray_ready(&self, ready: bool) {
        self.tray_ready.store(ready, Ordering::SeqCst);
    }

    /// Whether the tray icon exists.
    #[must_use]
    pub fn tray_ready(&self) -> bool {
        self.tray_ready.load(Ordering::SeqCst)
    }

    /// From now on a close request closes.
    pub fn begin_quit(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }

    /// Whether [`DesktopPrefs::begin_quit`] was called.
    #[must_use]
    pub fn quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    /// What a close request does now.
    #[must_use]
    pub fn decision(&self) -> CloseDecision {
        close_decision(self.close_to_tray(), self.tray_ready(), self.quitting())
    }
}

/// Brings the main window to the front: shown (it may be hidden in the tray), unminimised and focused. `false`
/// when there is no main window (the app is still starting; what asked for it is held and the window will show
/// anyway).
pub fn show_main_window<R: Runtime>(manager: &impl Manager<R>) -> bool {
    let Some(window) = manager.get_webview_window(MAIN_WINDOW) else {
        return false;
    };
    // Each step may be refused (a window that is closing); the others still help.
    if window.is_minimized().unwrap_or(false)
        && let Err(error) = window.unminimize()
    {
        tracing::debug!(%error, "the window could not be unminimised");
    }
    if let Err(error) = window.show() {
        tracing::debug!(%error, "the window could not be shown");
    }
    if let Err(error) = window.set_focus() {
        tracing::debug!(%error, "the window could not be focused");
    }
    true
}

/// What happens to a close request of the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The window was hidden and must not be closed.
    Prevent,
    /// The close goes ahead.
    Allow,
}

/// A close request of the main window: when [`DesktopPrefs::decision`] says hide, `hide` is called and the close
/// is prevented; a window that cannot be hidden is closed rather than left in front of a person who asked it to
/// go. (The window event handler is [`on_window_event`]; this is its logic, which the mock runtime cannot reach.)
pub fn on_close_requested(
    prefs: &DesktopPrefs,
    hide: impl FnOnce() -> tauri::Result<()>,
) -> CloseOutcome {
    if prefs.decision() != CloseDecision::Hide {
        return CloseOutcome::Allow;
    }
    match hide() {
        Ok(()) => {
            tracing::info!("the window was hidden to the tray");
            CloseOutcome::Prevent
        }
        Err(error) => {
            tracing::warn!(%error, "the window could not be hidden to the tray");
            CloseOutcome::Allow
        }
    }
}

/// The window event handler of the app: a close request of the main window goes through [`on_close_requested`],
/// and its loss of focus hides the menu bar. Every other event, and every other window, is left alone.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    if matches!(event, WindowEvent::Focused(false)) {
        // A bar left open behind another window would be there when the person comes back.
        crate::menu_bar::set_visible(window.app_handle(), false);
        return;
    }
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let Some(prefs) = window.try_state::<Arc<DesktopPrefs>>() else {
        return;
    };
    if on_close_requested(&prefs, || window.hide()) == CloseOutcome::Prevent {
        api.prevent_close();
    }
}

/// Quits the app the way closing its last window does (see the module's text).
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!("quitting from the tray");
    if let Some(prefs) = app.try_state::<Arc<DesktopPrefs>>() {
        prefs.begin_quit();
    }
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => {
            if let Err(error) = window.close() {
                tracing::warn!(%error, "the window could not be closed: exiting");
                app.exit(0);
            }
        }
        None => app.exit(0),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn closing_hides_only_when_everything_allows_it() {
        for close_to_tray in [false, true] {
            for tray_ready in [false, true] {
                for quitting in [false, true] {
                    let expected = if close_to_tray && tray_ready && !quitting {
                        CloseDecision::Hide
                    } else {
                        CloseDecision::Close
                    };
                    assert_eq!(
                        close_decision(close_to_tray, tray_ready, quitting),
                        expected,
                        "{close_to_tray} {tray_ready} {quitting}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_close_request_hides_the_window_and_prevents_the_close() {
        let prefs = DesktopPrefs::new();
        prefs.set_tray_ready(true);
        let mut hidden = 0;
        let outcome = on_close_requested(&prefs, || {
            hidden += 1;
            Ok(())
        });
        assert_eq!(outcome, CloseOutcome::Prevent);
        assert_eq!(hidden, 1);
    }

    #[test]
    fn a_close_request_that_is_not_a_hide_hides_nothing() {
        // The setting is off, there is no tray, or the app is quitting: the close goes ahead.
        for configure in [
            |prefs: &DesktopPrefs| prefs.set_close_to_tray(false),
            |prefs: &DesktopPrefs| prefs.set_tray_ready(false),
            DesktopPrefs::begin_quit,
        ] {
            let prefs = DesktopPrefs::new();
            prefs.set_tray_ready(true);
            configure(&prefs);
            let mut hidden = 0;
            let outcome = on_close_requested(&prefs, || {
                hidden += 1;
                Ok(())
            });
            assert_eq!(outcome, CloseOutcome::Allow);
            assert_eq!(hidden, 0);
        }
    }

    #[test]
    fn a_window_that_cannot_be_hidden_is_closed() {
        let prefs = DesktopPrefs::new();
        prefs.set_tray_ready(true);
        let outcome = on_close_requested(&prefs, || Err(tauri::Error::WindowNotFound));
        assert_eq!(outcome, CloseOutcome::Allow);
    }

    #[test]
    fn the_defaults_hide_once_there_is_a_tray() {
        let prefs = DesktopPrefs::new();
        // No tray icon yet: a close request closes, so a window is never hidden with nothing to bring it back.
        assert!(prefs.close_to_tray());
        assert!(!prefs.tray_ready());
        assert!(!prefs.quitting());
        assert_eq!(prefs.decision(), CloseDecision::Close);

        prefs.set_tray_ready(true);
        assert_eq!(prefs.decision(), CloseDecision::Hide);
    }

    #[test]
    fn the_setting_and_the_tray_and_a_quit_each_change_the_decision() {
        let prefs = DesktopPrefs::new();
        prefs.set_tray_ready(true);
        assert_eq!(prefs.decision(), CloseDecision::Hide);

        prefs.set_close_to_tray(false);
        assert_eq!(prefs.decision(), CloseDecision::Close);
        prefs.set_close_to_tray(true);
        assert_eq!(prefs.decision(), CloseDecision::Hide);

        prefs.set_tray_ready(false);
        assert_eq!(prefs.decision(), CloseDecision::Close);
        prefs.set_tray_ready(true);

        prefs.begin_quit();
        assert!(prefs.quitting());
        assert_eq!(prefs.decision(), CloseDecision::Close);
    }
}
