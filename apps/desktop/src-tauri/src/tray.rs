//! The tray icon (ADR-0040): the app's icon in the notification area with the tooltip "atomik-kb" and a menu of
//! Open, New page, Lock and Quit. A left click on the icon shows, unminimises and focuses the main window (the
//! menu opens on a right click). What each item does is `desktop::perform`; this module builds the icon and
//! turns the clicks into [`TrayAction`]s.
//!
//! The tray is built from Rust only (`tauri`'s `tray-icon` feature), so the webview's capability gains nothing.
//! The labels come from `shell_labels` (English until the UI sends its own) and are changed with
//! [`Tray::set_labels`]. On Linux the icon needs a status-notifier host and `libayatana-appindicator3` at run
//! time (`docs/development/packaging.md`); when the tray cannot be built the app runs without one and the close
//! button closes (`window`).
//!
//! The mock runtime of the tests does not run an operating system's notification area, so what is tested is
//! what surrounds it: the id-to-action mapping, the labels and the actions (`tests/tray.rs`); the icon itself,
//! its menu and its click are checked in the real-app pass.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Runtime};

use crate::shell_labels::Labels;

/// The id of the one tray icon.
pub const TRAY_ID: &str = "atomik-kb";

/// The tooltip of the icon: the app's name, which no one translates.
pub const TOOLTIP: &str = "atomik-kb";

/// Menu item ids (what a [`MenuEvent`] carries).
pub const OPEN_ID: &str = "tray-open";
/// See [`OPEN_ID`].
pub const NEW_PAGE_ID: &str = "tray-new-page";
/// See [`OPEN_ID`].
pub const LOCK_ID: &str = "tray-lock";
/// See [`OPEN_ID`].
pub const QUIT_ID: &str = "tray-quit";

/// What a menu item asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Show the window.
    Open,
    /// Show the window and start a page in it.
    NewPage,
    /// Lock the app.
    Lock,
    /// End the app.
    Quit,
}

/// The action of the menu item with this id; `None` for any other id.
#[must_use]
pub fn action_of(id: &str) -> Option<TrayAction> {
    match id {
        OPEN_ID => Some(TrayAction::Open),
        NEW_PAGE_ID => Some(TrayAction::NewPage),
        LOCK_ID => Some(TrayAction::Lock),
        QUIT_ID => Some(TrayAction::Quit),
        _ => None,
    }
}

/// Whether a click on the icon itself (not on its menu) brings the window back: the left button, on release.
#[must_use]
pub fn is_show_click(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

/// The icon and the four items whose text can change. Dropping it removes the icon, so the app keeps it in its
/// managed state for as long as it runs.
pub struct Tray<R: Runtime> {
    _icon: TrayIcon<R>,
    open: MenuItem<R>,
    new_page: MenuItem<R>,
    lock: MenuItem<R>,
    quit: MenuItem<R>,
}

impl<R: Runtime> Tray<R> {
    /// Builds the icon (the app's own, from the bundle) and its menu with `labels`.
    ///
    /// # Errors
    ///
    /// When the menu or the icon cannot be built (no status-notifier host on Linux, no default icon).
    pub fn build(app: &AppHandle<R>, labels: &Labels) -> tauri::Result<Self> {
        let open = MenuItem::with_id(app, OPEN_ID, &labels.open, true, None::<&str>)?;
        let new_page = MenuItem::with_id(app, NEW_PAGE_ID, &labels.new_page, true, None::<&str>)?;
        let lock = MenuItem::with_id(app, LOCK_ID, &labels.lock, true, None::<&str>)?;
        let quit = MenuItem::with_id(app, QUIT_ID, &labels.quit, true, None::<&str>)?;
        let separator = PredefinedMenuItem::separator(app)?;
        let menu = Menu::with_items(app, &[&open, &new_page, &lock, &separator, &quit])?;
        let mut builder = TrayIconBuilder::with_id(TRAY_ID)
            .tooltip(TOOLTIP)
            .menu(&menu)
            // The left click shows the window; the menu is on the right button.
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event: MenuEvent| {
                if let Some(action) = action_of(event.id().as_ref()) {
                    crate::desktop::perform(app, action);
                }
            })
            .on_tray_icon_event(|tray, event| {
                if is_show_click(&event) {
                    crate::window::show_main_window(tray.app_handle());
                }
            });
        match app.default_window_icon() {
            Some(icon) => builder = builder.icon(icon.clone()),
            None => tracing::warn!("the app has no default icon: the tray icon has none either"),
        }
        let icon = builder.build(app)?;
        Ok(Self {
            _icon: icon,
            open,
            new_page,
            lock,
            quit,
        })
    }

    /// Changes the text of the four items.
    ///
    /// # Errors
    ///
    /// When the platform refuses to change an item; the others were still changed.
    pub fn set_labels(&self, labels: &Labels) -> tauri::Result<()> {
        let results = [
            self.open.set_text(&labels.open),
            self.new_page.set_text(&labels.new_page),
            self.lock.set_text(&labels.lock),
            self.quit.set_text(&labels.quit),
        ];
        results.into_iter().collect()
    }

    /// The texts of the four items now, in menu order: Open, New page, Lock, Quit.
    ///
    /// # Errors
    ///
    /// When the platform cannot read an item.
    pub fn texts(&self) -> tauri::Result<[String; 4]> {
        Ok([
            self.open.text()?,
            self.new_page.text()?,
            self.lock.text()?,
            self.quit.text()?,
        ])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_item_id_maps_to_its_action() {
        assert_eq!(action_of(OPEN_ID), Some(TrayAction::Open));
        assert_eq!(action_of(NEW_PAGE_ID), Some(TrayAction::NewPage));
        assert_eq!(action_of(LOCK_ID), Some(TrayAction::Lock));
        assert_eq!(action_of(QUIT_ID), Some(TrayAction::Quit));
    }

    #[test]
    fn the_ids_are_distinct_and_nothing_else_is_an_action() {
        let ids = [OPEN_ID, NEW_PAGE_ID, LOCK_ID, QUIT_ID];
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[..index].contains(id), "{id} is used twice");
        }
        for other in [
            "",
            "open",
            "tray-",
            "tray-open ",
            "TRAY-OPEN",
            "quit",
            "lock",
        ] {
            assert_eq!(action_of(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_tooltip_is_the_apps_name() {
        assert_eq!(TOOLTIP, "atomik-kb");
    }

    fn rect() -> tauri::Rect {
        tauri::Rect {
            position: tauri::PhysicalPosition::new(0, 0).into(),
            size: tauri::PhysicalSize::new(16, 16).into(),
        }
    }

    fn click(button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: TRAY_ID.into(),
            position: tauri::PhysicalPosition::new(0.0, 0.0),
            rect: rect(),
            button,
            button_state,
        }
    }

    #[test]
    fn only_the_release_of_a_left_click_brings_the_window_back() {
        assert!(is_show_click(&click(
            MouseButton::Left,
            MouseButtonState::Up
        )));
        // The press, the other buttons and the other events do not: the right button opens the menu, and a
        // click is shown once, not once for its press and once for its release.
        assert!(!is_show_click(&click(
            MouseButton::Left,
            MouseButtonState::Down
        )));
        assert!(!is_show_click(&click(
            MouseButton::Right,
            MouseButtonState::Up
        )));
        assert!(!is_show_click(&click(
            MouseButton::Middle,
            MouseButtonState::Up
        )));
        let id = || TRAY_ID.into();
        let position = || tauri::PhysicalPosition::new(0.0, 0.0);
        for other in [
            TrayIconEvent::Enter {
                id: id(),
                position: position(),
                rect: rect(),
            },
            TrayIconEvent::Move {
                id: id(),
                position: position(),
                rect: rect(),
            },
            TrayIconEvent::Leave {
                id: id(),
                position: position(),
                rect: rect(),
            },
            TrayIconEvent::DoubleClick {
                id: id(),
                position: position(),
                rect: rect(),
                button: MouseButton::Left,
            },
        ] {
            assert!(!is_show_click(&other), "{other:?}");
        }
    }
}
