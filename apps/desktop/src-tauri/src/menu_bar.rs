//! The native menu bar (ADR-0040 item 12): File, Edit, View, Window and Help on the main window, hidden until a
//! lone Alt press in the webview (the UI detects it, `menu-bar.ts`, and calls `menu_bar_show`). It hides again
//! after a choice, when the UI says Escape (`menu_bar_hide`), when the window loses focus (`window`) and when the
//! page reloads (`lib.rs`). Nothing here is an always-visible fallback: a bar that cannot be shown or hidden
//! cleanly is a reason to stop, not to ship (ADR-0040). A second Alt press while the bar is shown is not ours: the
//! operating system's own menu mode takes it (the first menu lit, the arrows move), see [`show`].
//!
//! ```text
//!   Alt, alone, in the webview ──► menu_bar_show ──► Window::show_menu ──► `menuBar { visible }` to the UI
//!   a menu item ──► on_menu_event ──► hide the bar ──► perform(Item) ──► the shell, or `menu { id }` to the UI
//! ```
//!
//! **What an item does.** Every item is an [`Item`] with one [`Action`]: *New page* and *Lock* are the tray's
//! (`desktop::capture`, `desktop::lock`), *Quit* is the tray's Quit (`window::quit`), *Minimize* and *Close
//! window* are the shell's own (closing goes through the window's close request, so it follows close to the
//! tray), and the rest (import, export, print, find, toggle the sidebar, back, forward, about, and undo and redo
//! where they are not native) are a `menu` message to the UI, which runs its existing action
//! (`shell_events`); an action the UI cannot run now (an export with no page open) does nothing harmful.
//!
//! **Edit.** Cut, Copy, Paste and Select all are muda's predefined items on every platform: on Windows muda runs
//! them by sending Ctrl+X, C, V and A to the focused window (`SendInput`), on GTK3 through the focused widget,
//! so they work in the title field, in the editor and in the page's text alike, and Paste needs no clipboard
//! permission. Undo and Redo are predefined on Windows (muda 0.20 sends Ctrl+Z and Ctrl+Y, which the editor
//! already binds to its Loro undo) and are UI-driven items elsewhere, because muda's GTK3 build lists them
//! disabled ([`Capabilities::native_history`]). A predefined item emits no menu event, so a choice of one does
//! not hide the bar here: the UI hides it on the next key or click in the page, which the key that muda sends
//! is.
//!
//! **Shortcut hints.** An item shows the real shortcut of the UI beside its label (`Ctrl+P`, `Alt+Left`), and the
//! hint is **text after a tab in the label, never an accelerator**: muda registers an accelerator in the
//! window's accelerator table, which Tauri feeds with every message of the event loop
//! (`TranslateAcceleratorW`), so a registered `Ctrl+P` could fire on its own and again in the webview. The
//! hints are composed here from fixed strings (the UI's labels may not carry a tab, `shell_labels`), only where
//! the platform's menus draw a tab as a column ([`Capabilities::hints`]: Windows). The one exception is not
//! ours to choose: muda gives its predefined Edit items their usual accelerators (Ctrl+X, C, V, A, Z, Y) on
//! Windows, which show as their hints. Keys typed in the webview do not pass the window's message loop (WebView2
//! keeps them in its own process), so they are not translated twice; the real-app pass checks each of them.
//!
//! **Labels.** The text of every item comes from the UI through `shell_labels_set` (English until it says
//! otherwise) and is changed in place with `set_text` ([`MenuBar::set_labels`]). A label may carry `&` for a
//! mnemonic. The mock runtime has no menu bar on screen: the tests cover the description ([`describe`]), the
//! actions and the commands, and that the menu builds with the labels; showing, hiding and the native items are
//! checked in the real-app pass.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};

use crate::shell_events::ShellEvents;
use crate::shell_labels::{LabelStore, Labels};
use crate::{MAIN_WINDOW, desktop, window};

/// The id of the menu itself.
pub const MENU_ID: &str = "menu-bar";

/// The hints beside the labels: the keys the UI really binds (`shortcuts.ts`, `MainLayout`, `print.ts`,
/// `FindBar`), and Windows' own for closing a window. Text only: they are never registered as accelerators.
pub const HINT_PRINT: &str = "Ctrl+P";
/// See [`HINT_PRINT`].
pub const HINT_FIND: &str = "Ctrl+F";
/// See [`HINT_PRINT`].
pub const HINT_TOGGLE_SIDEBAR: &str = "Ctrl+\\";
/// See [`HINT_PRINT`].
pub const HINT_BACK: &str = "Alt+Left";
/// See [`HINT_PRINT`].
pub const HINT_FORWARD: &str = "Alt+Right";
/// See [`HINT_PRINT`].
pub const HINT_CLOSE_WINDOW: &str = "Alt+F4";

/// A menu item the shell makes itself (everything but the predefined Edit items of [`Native`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// File: a new page.
    NewPage,
    /// File: import into the current space.
    Import,
    /// File: export the open page, or the current space.
    Export,
    /// File: print the open page.
    Print,
    /// File: lock the app.
    Lock,
    /// File: quit.
    Quit,
    /// Edit: undo (UI-driven where it is not native).
    Undo,
    /// Edit: redo (UI-driven where it is not native).
    Redo,
    /// Edit: find in the open page.
    Find,
    /// View: show or hide the sidebar.
    ToggleSidebar,
    /// View: back in the active tab's history.
    Back,
    /// View: forward in the active tab's history.
    Forward,
    /// Window: minimize.
    Minimize,
    /// Window: close the window.
    CloseWindow,
    /// Help: about.
    About,
}

impl Item {
    /// Every item.
    pub const ALL: [Self; 15] = [
        Self::NewPage,
        Self::Import,
        Self::Export,
        Self::Print,
        Self::Lock,
        Self::Quit,
        Self::Undo,
        Self::Redo,
        Self::Find,
        Self::ToggleSidebar,
        Self::Back,
        Self::Forward,
        Self::Minimize,
        Self::CloseWindow,
        Self::About,
    ];

    /// The id of the item in the menu (what a [`MenuEvent`] carries).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::NewPage => "menu-new-page",
            Self::Import => "menu-import",
            Self::Export => "menu-export",
            Self::Print => "menu-print",
            Self::Lock => "menu-lock",
            Self::Quit => "menu-quit",
            Self::Undo => "menu-undo",
            Self::Redo => "menu-redo",
            Self::Find => "menu-find",
            Self::ToggleSidebar => "menu-toggle-sidebar",
            Self::Back => "menu-back",
            Self::Forward => "menu-forward",
            Self::Minimize => "menu-minimize",
            Self::CloseWindow => "menu-close-window",
            Self::About => "menu-about",
        }
    }

    /// What choosing the item does.
    #[must_use]
    pub const fn action(self) -> Action {
        match self {
            Self::NewPage => Action::Capture,
            Self::Lock => Action::Lock,
            Self::Quit => Action::Quit,
            Self::Minimize => Action::Minimize,
            Self::CloseWindow => Action::CloseWindow,
            Self::Import => Action::Ui(UiCommand::Import),
            Self::Export => Action::Ui(UiCommand::Export),
            Self::Print => Action::Ui(UiCommand::Print),
            Self::Undo => Action::Ui(UiCommand::Undo),
            Self::Redo => Action::Ui(UiCommand::Redo),
            Self::Find => Action::Ui(UiCommand::Find),
            Self::ToggleSidebar => Action::Ui(UiCommand::ToggleSidebar),
            Self::Back => Action::Ui(UiCommand::Back),
            Self::Forward => Action::Ui(UiCommand::Forward),
            Self::About => Action::Ui(UiCommand::About),
        }
    }

    /// The shortcut the UI binds to the same thing, as text to show beside the label.
    #[must_use]
    pub const fn hint(self) -> Option<&'static str> {
        match self {
            Self::Print => Some(HINT_PRINT),
            Self::Find => Some(HINT_FIND),
            Self::ToggleSidebar => Some(HINT_TOGGLE_SIDEBAR),
            Self::Back => Some(HINT_BACK),
            Self::Forward => Some(HINT_FORWARD),
            Self::CloseWindow => Some(HINT_CLOSE_WINDOW),
            Self::NewPage
            | Self::Import
            | Self::Export
            | Self::Lock
            | Self::Quit
            | Self::Undo
            | Self::Redo
            | Self::Minimize
            | Self::About => None,
        }
    }
}

/// The item whose id this is; `None` for any other id (the tray's, a predefined item's).
#[must_use]
pub fn item_of(id: &str) -> Option<Item> {
    Item::ALL.into_iter().find(|item| item.id() == id)
}

/// What the UI is asked to run: the `id` of a `menu` message of `shell_events_subscribe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UiCommand {
    /// Import into the current space.
    Import,
    /// Export the open page, or the current space.
    Export,
    /// Print the open page.
    Print,
    /// Undo (in the editor, else the sidebar's structural undo).
    Undo,
    /// Redo.
    Redo,
    /// Find in the open page.
    Find,
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// Back in the active tab's history.
    Back,
    /// Forward in the active tab's history.
    Forward,
    /// The About dialog.
    About,
}

/// What choosing an item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A quick capture: the window comes to the front and the UI makes a page (`desktop::capture`).
    Capture,
    /// The app lock, the UI's own action (`desktop::lock`).
    Lock,
    /// Quit the way the tray's Quit does (`window::quit`).
    Quit,
    /// Minimize the main window.
    Minimize,
    /// Close the main window: a close request, so it follows close to the tray.
    CloseWindow,
    /// Ask the UI.
    Ui(UiCommand),
}

/// The Edit items muda runs itself (see the module's text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Cut.
    Cut,
    /// Copy.
    Copy,
    /// Paste.
    Paste,
    /// Select all.
    SelectAll,
}

/// What the platform's menus can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// A tab in a label draws the text after it as a right-aligned shortcut column (Win32 menus).
    pub hints: bool,
    /// muda runs Undo and Redo itself and enabled (Windows); elsewhere they are items of ours.
    pub native_history: bool,
}

impl Capabilities {
    /// What this build's platform has.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            hints: cfg!(windows),
            native_history: cfg!(windows),
        }
    }
}

/// One entry of a menu, described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A line between groups.
    Separator,
    /// An item of ours.
    Item {
        /// Which.
        item: Item,
        /// The UI's words for it.
        label: String,
        /// The shortcut to show beside it, when the platform draws one.
        hint: Option<&'static str>,
    },
    /// An item muda runs.
    Native {
        /// Which.
        kind: Native,
        /// The UI's words for it.
        label: String,
    },
}

impl Entry {
    fn item(item: Item, label: &str, caps: Capabilities) -> Self {
        Self::Item {
            item,
            label: label.to_owned(),
            hint: if caps.hints { item.hint() } else { None },
        }
    }

    fn native(kind: Native, label: &str) -> Self {
        Self::Native {
            kind,
            label: label.to_owned(),
        }
    }

    /// The label alone, as `MenuItem::text` answers it; `None` for a separator.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Separator => None,
            Self::Item { label, .. } | Self::Native { label, .. } => Some(label),
        }
    }

    /// The text given to the platform: the label, and after a tab the hint where there is one.
    #[must_use]
    pub fn shown(&self) -> Option<String> {
        match self {
            Self::Separator => None,
            Self::Item {
                label,
                hint: Some(hint),
                ..
            } => Some(format!("{label}\t{hint}")),
            Self::Item { label, .. } | Self::Native { label, .. } => Some(label.clone()),
        }
    }
}

/// One menu of the bar, described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The id of the submenu.
    pub id: &'static str,
    /// The UI's words for it.
    pub label: String,
    /// Its entries, top to bottom.
    pub entries: Vec<Entry>,
}

/// The whole bar for these labels: File, Edit, View, Window and Help (pure; [`MenuBar::build`] makes it real).
#[must_use]
pub fn describe(labels: &Labels, caps: Capabilities) -> Vec<Section> {
    let item = |item, label: &str| Entry::item(item, label, caps);
    let history = |item: Item, native: Native, label: &str| {
        if caps.native_history {
            Entry::native(native, label)
        } else {
            Entry::item(item, label, caps)
        }
    };
    vec![
        Section {
            id: "menu-file",
            label: labels.menu_file.clone(),
            entries: vec![
                item(Item::NewPage, &labels.menu_new_page),
                Entry::Separator,
                item(Item::Import, &labels.menu_import),
                item(Item::Export, &labels.menu_export),
                item(Item::Print, &labels.menu_print),
                Entry::Separator,
                item(Item::Lock, &labels.menu_lock),
                Entry::Separator,
                item(Item::Quit, &labels.menu_quit),
            ],
        },
        Section {
            id: "menu-edit",
            label: labels.menu_edit.clone(),
            entries: vec![
                history(Item::Undo, Native::Undo, &labels.menu_undo),
                history(Item::Redo, Native::Redo, &labels.menu_redo),
                Entry::Separator,
                Entry::native(Native::Cut, &labels.menu_cut),
                Entry::native(Native::Copy, &labels.menu_copy),
                Entry::native(Native::Paste, &labels.menu_paste),
                Entry::native(Native::SelectAll, &labels.menu_select_all),
                Entry::Separator,
                item(Item::Find, &labels.menu_find),
            ],
        },
        Section {
            id: "menu-view",
            label: labels.menu_view.clone(),
            entries: vec![
                item(Item::ToggleSidebar, &labels.menu_toggle_sidebar),
                Entry::Separator,
                item(Item::Back, &labels.menu_back),
                item(Item::Forward, &labels.menu_forward),
            ],
        },
        Section {
            id: "menu-window",
            label: labels.menu_window.clone(),
            entries: vec![
                item(Item::Minimize, &labels.menu_minimize),
                item(Item::CloseWindow, &labels.menu_close_window),
            ],
        },
        Section {
            id: "menu-help",
            label: labels.menu_help.clone(),
            entries: vec![item(Item::About, &labels.menu_about)],
        },
    ]
}

/// The labels of a description in the order [`MenuBar`] keeps its handles: each menu's own, then its entries'
/// (separators have none).
#[must_use]
pub fn labels_of(sections: &[Section]) -> Vec<String> {
    sections
        .iter()
        .flat_map(|section| {
            std::iter::once(section.label.clone()).chain(
                section
                    .entries
                    .iter()
                    .filter_map(|e| e.label().map(str::to_owned)),
            )
        })
        .collect()
}

/// The texts of a description as the platform gets them (labels with their hints), in the order of
/// [`labels_of`].
#[must_use]
pub fn shown_of(sections: &[Section]) -> Vec<String> {
    sections
        .iter()
        .flat_map(|section| {
            std::iter::once(section.label.clone())
                .chain(section.entries.iter().filter_map(Entry::shown))
        })
        .collect()
}

/// One item of the real menu whose text can change.
enum Handle<R: Runtime> {
    Submenu(Submenu<R>),
    Item(MenuItem<R>),
    Native(PredefinedMenuItem<R>),
}

impl<R: Runtime> Handle<R> {
    fn set_text(&self, text: &str) -> tauri::Result<()> {
        match self {
            Self::Submenu(menu) => menu.set_text(text),
            Self::Item(item) => item.set_text(text),
            Self::Native(item) => item.set_text(text),
        }
    }

    /// The label now (without a hint).
    fn label(&self) -> tauri::Result<String> {
        match self {
            Self::Submenu(menu) => menu.text(),
            Self::Item(item) => item.text(),
            Self::Native(item) => item.text(),
        }
    }
}

fn native_item<R: Runtime>(
    app: &AppHandle<R>,
    kind: Native,
    label: &str,
) -> tauri::Result<PredefinedMenuItem<R>> {
    let text = Some(label);
    match kind {
        Native::Undo => PredefinedMenuItem::undo(app, text),
        Native::Redo => PredefinedMenuItem::redo(app, text),
        Native::Cut => PredefinedMenuItem::cut(app, text),
        Native::Copy => PredefinedMenuItem::copy(app, text),
        Native::Paste => PredefinedMenuItem::paste(app, text),
        Native::SelectAll => PredefinedMenuItem::select_all(app, text),
    }
}

/// The real menu bar and whether it is on the window now. Managed by Tauri; dropping it would drop the menu.
pub struct MenuBar<R: Runtime> {
    menu: Menu<R>,
    handles: Vec<Handle<R>>,
    caps: Capabilities,
    visible: AtomicBool,
}

impl<R: Runtime> MenuBar<R> {
    /// Builds the menu for `labels` (not on any window yet, and not visible).
    ///
    /// # Errors
    ///
    /// When the platform cannot make an item.
    pub fn build(app: &AppHandle<R>, labels: &Labels, caps: Capabilities) -> tauri::Result<Self> {
        let menu = Menu::with_id(app, MENU_ID)?;
        let mut handles = Vec::new();
        for section in describe(labels, caps) {
            let submenu = Submenu::with_id(app, section.id, &section.label, true)?;
            handles.push(Handle::Submenu(submenu.clone()));
            for entry in &section.entries {
                match entry {
                    Entry::Separator => submenu.append(&PredefinedMenuItem::separator(app)?)?,
                    Entry::Item { item, .. } => {
                        let shown = entry.shown().unwrap_or_default();
                        let made = MenuItem::with_id(app, item.id(), shown, true, None::<&str>)?;
                        submenu.append(&made)?;
                        handles.push(Handle::Item(made));
                    }
                    Entry::Native { kind, label } => {
                        let made = native_item(app, *kind, label)?;
                        submenu.append(&made)?;
                        handles.push(Handle::Native(made));
                    }
                }
            }
            menu.append(&submenu)?;
        }
        Ok(Self {
            menu,
            handles,
            caps,
            visible: AtomicBool::new(false),
        })
    }

    /// The menu, to put on a window.
    #[must_use]
    pub fn menu(&self) -> &Menu<R> {
        &self.menu
    }

    /// Whether the bar is on the window now, as far as the shell knows.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::SeqCst)
    }

    /// Changes the text of every item in place.
    ///
    /// # Errors
    ///
    /// When the platform refuses to change an item; the others were still changed.
    pub fn set_labels(&self, labels: &Labels) -> tauri::Result<()> {
        let texts = shown_of(&describe(labels, self.caps));
        let mut result = Ok(());
        for (handle, text) in self.handles.iter().zip(texts) {
            // One refusal does not keep the others from changing.
            if let Err(error) = handle.set_text(&text)
                && result.is_ok()
            {
                result = Err(error);
            }
        }
        result
    }

    /// The label of every item now, in the order of [`labels_of`].
    ///
    /// # Errors
    ///
    /// When the platform cannot read an item.
    pub fn labels(&self) -> tauri::Result<Vec<String>> {
        self.handles.iter().map(Handle::label).collect()
    }
}

/// Builds the menu bar with the labels of the moment, puts it on the main window and hides it at once (before
/// the window's first paint, since this runs in `setup`), and keeps it in the managed state. `false` when it
/// could not be made (logged; the app runs without a menu bar). macOS has an app-wide menu instead, which this
/// does not build: macOS is not built (ADR-0040).
pub fn install<R: Runtime>(app: &AppHandle<R>) -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    let labels = app
        .try_state::<Arc<LabelStore>>()
        .map(|store| store.get())
        .unwrap_or_default();
    let bar = match MenuBar::build(app, &labels, Capabilities::current()) {
        Ok(bar) => bar,
        Err(error) => {
            tracing::warn!(%error, "the menu bar could not be built");
            return false;
        }
    };
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!("there is no main window to put the menu bar on");
        return false;
    };
    if let Err(error) = window.set_menu(bar.menu().clone()) {
        tracing::warn!(%error, "the menu bar could not be put on the window");
        return false;
    }
    // Hidden from the start: only a lone Alt shows it.
    if let Err(error) = window.hide_menu() {
        tracing::warn!(%error, "the menu bar could not be hidden");
    }
    app.manage(bar);
    tracing::info!("the menu bar is ready, hidden until Alt");
    true
}

/// Shows or hides the bar on the main window and tells the UI. `true` when the state changed; `false` when it
/// was already so, when there is no menu bar or window, or when the platform refused (the state then stays).
pub fn set_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) -> bool {
    let Some(bar) = app.try_state::<MenuBar<R>>() else {
        return false;
    };
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return false;
    };
    if bar.visible.swap(visible, Ordering::SeqCst) == visible {
        return false;
    }
    let result = if visible {
        window.show_menu()
    } else {
        window.hide_menu()
    };
    if let Err(error) = result {
        tracing::warn!(%error, "the menu bar could not be {}", if visible { "shown" } else { "hidden" });
        bar.visible.store(!visible, Ordering::SeqCst);
        return false;
    }
    if let Some(events) = app.try_state::<Arc<ShellEvents>>() {
        events.menu_bar(visible);
    }
    true
}

/// `menu_bar_show`: shows the bar (the lone Alt press). `true` when it was hidden and is shown now. A bar that is
/// shown already is left alone, and so is a second Alt press: it is the operating system's, which starts the
/// menu's own keyboard mode (the first menu lit, the arrows move) while the bar is on the window, and taking the menu
/// off the window under it would end that mode.
pub fn show<R: Runtime>(app: &AppHandle<R>) -> bool {
    set_visible(app, true)
}

/// What the app does with a menu event: a choice of one of our items hides the bar and runs the item; any
/// other id (the tray's, a predefined item's) is not ours.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    let Some(item) = item_of(event.id().as_ref()) else {
        return;
    };
    set_visible(app, false);
    perform(app, item);
}

/// What choosing `item` does (see the module's text).
pub fn perform<R: Runtime>(app: &AppHandle<R>, item: Item) {
    match item.action() {
        Action::Capture => desktop::capture(app),
        Action::Lock => desktop::lock(app),
        Action::Quit => window::quit(app),
        Action::Minimize => {
            if let Some(main) = app.get_webview_window(MAIN_WINDOW)
                && let Err(error) = main.minimize()
            {
                tracing::debug!(%error, "the window could not be minimized");
            }
        }
        Action::CloseWindow => {
            if let Some(main) = app.get_webview_window(MAIN_WINDOW)
                && let Err(error) = main.close()
            {
                tracing::debug!(%error, "the window could not be closed");
            }
        }
        Action::Ui(command) => {
            if let Some(events) = app.try_state::<Arc<ShellEvents>>() {
                events.menu(command);
            }
        }
    }
}

/// Puts the labels the UI sent on the menu bar, when there is one.
pub fn apply_labels<R: Runtime>(app: &AppHandle<R>, labels: &Labels) {
    if let Some(bar) = app.try_state::<MenuBar<R>>()
        && bar.set_labels(labels).is_err()
    {
        // Never the text: a label is the person's language.
        tracing::warn!("a menu bar label could not be changed");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const WINDOWS: Capabilities = Capabilities {
        hints: true,
        native_history: true,
    };
    const GTK: Capabilities = Capabilities {
        hints: false,
        native_history: false,
    };

    fn english(caps: Capabilities) -> Vec<Section> {
        describe(&Labels::default(), caps)
    }

    fn section<'a>(sections: &'a [Section], id: &str) -> &'a Section {
        sections.iter().find(|section| section.id == id).unwrap()
    }

    /// The `&` mnemonic letter of a label, lower-cased (`&&` is a literal ampersand).
    fn mnemonic(label: &str) -> Option<char> {
        let mut chars = label.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '&' {
                match chars.next() {
                    Some('&') | None => {}
                    Some(letter) => return letter.to_lowercase().next(),
                }
            }
        }
        None
    }

    // ----- items -----------------------------------------------------------------------------------------------

    #[test]
    fn every_item_id_maps_back_to_its_item() {
        for item in Item::ALL {
            assert_eq!(item_of(item.id()), Some(item), "{item:?}");
        }
    }

    #[test]
    fn the_ids_are_distinct_and_nothing_else_is_an_item() {
        let ids: Vec<&str> = Item::ALL.iter().map(|item| item.id()).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        for item in Item::ALL {
            assert!(item.id().starts_with("menu-"), "{item:?}");
        }
        for other in [
            "",
            "menu",
            "menu-",
            "menu-bar",
            "menu-file",
            "menu-print ",
            "MENU-PRINT",
            // The tray's ids are not ours.
            "tray-quit",
            "tray-lock",
            "quit",
        ] {
            assert_eq!(item_of(other), None, "{other:?}");
        }
        // The ids of the sections and the menu are not items either.
        for section in english(WINDOWS) {
            assert_eq!(item_of(section.id), None, "{}", section.id);
        }
    }

    #[test]
    fn the_items_the_shell_does_itself_are_new_page_lock_quit_minimize_and_close() {
        assert_eq!(Item::NewPage.action(), Action::Capture);
        assert_eq!(Item::Lock.action(), Action::Lock);
        assert_eq!(Item::Quit.action(), Action::Quit);
        assert_eq!(Item::Minimize.action(), Action::Minimize);
        assert_eq!(Item::CloseWindow.action(), Action::CloseWindow);
    }

    #[test]
    fn every_other_item_asks_the_ui_with_its_own_command() {
        let commands: Vec<UiCommand> = Item::ALL
            .iter()
            .filter_map(|item| match item.action() {
                Action::Ui(command) => Some(command),
                _ => None,
            })
            .collect();
        assert_eq!(
            commands,
            vec![
                UiCommand::Import,
                UiCommand::Export,
                UiCommand::Print,
                UiCommand::Undo,
                UiCommand::Redo,
                UiCommand::Find,
                UiCommand::ToggleSidebar,
                UiCommand::Back,
                UiCommand::Forward,
                UiCommand::About,
            ]
        );
    }

    #[test]
    fn the_commands_go_to_the_ui_in_camel_case() {
        let wire = |command: UiCommand| serde_json::to_value(command).unwrap();
        for (command, text) in [
            (UiCommand::Import, "import"),
            (UiCommand::Export, "export"),
            (UiCommand::Print, "print"),
            (UiCommand::Undo, "undo"),
            (UiCommand::Redo, "redo"),
            (UiCommand::Find, "find"),
            (UiCommand::ToggleSidebar, "toggleSidebar"),
            (UiCommand::Back, "back"),
            (UiCommand::Forward, "forward"),
            (UiCommand::About, "about"),
        ] {
            assert_eq!(wire(command), serde_json::json!(text));
        }
    }

    // ----- the description -------------------------------------------------------------------------------------

    #[test]
    fn the_bar_is_file_edit_view_window_and_help_in_that_order() {
        let sections = english(WINDOWS);
        let ids: Vec<&str> = sections.iter().map(|section| section.id).collect();
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
        let labels: Vec<&str> = sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["&File", "&Edit", "&View", "&Window", "&Help"]);
    }

    fn items_of(section: &Section) -> Vec<Item> {
        section
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Item { item, .. } => Some(*item),
                _ => None,
            })
            .collect()
    }

    fn natives_of(section: &Section) -> Vec<Native> {
        section
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Native { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_items_are_the_ones_of_the_adr() {
        let sections = english(WINDOWS);
        assert_eq!(
            items_of(section(&sections, "menu-file")),
            vec![
                Item::NewPage,
                Item::Import,
                Item::Export,
                Item::Print,
                Item::Lock,
                Item::Quit
            ]
        );
        assert_eq!(
            items_of(section(&sections, "menu-view")),
            vec![Item::ToggleSidebar, Item::Back, Item::Forward]
        );
        assert_eq!(
            items_of(section(&sections, "menu-window")),
            vec![Item::Minimize, Item::CloseWindow]
        );
        assert_eq!(items_of(section(&sections, "menu-help")), vec![Item::About]);
        // Every item of ours is in exactly one menu (undo and redo are Edit's, native or not).
        let mut seen: Vec<Item> = Vec::new();
        for platform in [WINDOWS, GTK] {
            seen.clear();
            for section in &describe(&Labels::default(), platform) {
                seen.extend(items_of(section));
            }
            let expected: Vec<Item> = Item::ALL
                .into_iter()
                .filter(|item| {
                    !(platform.native_history && matches!(item, Item::Undo | Item::Redo))
                })
                .collect();
            let mut sorted = seen.clone();
            sorted.sort_by_key(|item| item.id());
            let mut want = expected;
            want.sort_by_key(|item| item.id());
            assert_eq!(sorted, want, "{platform:?}");
        }
    }

    #[test]
    fn edit_uses_the_native_items_and_find_is_ours() {
        let sections = english(WINDOWS);
        let edit = section(&sections, "menu-edit");
        assert_eq!(
            natives_of(edit),
            vec![
                Native::Undo,
                Native::Redo,
                Native::Cut,
                Native::Copy,
                Native::Paste,
                Native::SelectAll
            ]
        );
        assert_eq!(items_of(edit), vec![Item::Find]);
    }

    #[test]
    fn where_undo_and_redo_are_not_native_they_are_items_of_ours() {
        let sections = english(GTK);
        let edit = section(&sections, "menu-edit");
        assert_eq!(
            natives_of(edit),
            vec![Native::Cut, Native::Copy, Native::Paste, Native::SelectAll]
        );
        assert_eq!(items_of(edit), vec![Item::Undo, Item::Redo, Item::Find]);
        // In the same place: first in the menu.
        assert!(matches!(
            edit.entries.first(),
            Some(Entry::Item {
                item: Item::Undo,
                ..
            })
        ));
    }

    #[test]
    fn separators_are_between_groups_never_first_last_or_doubled() {
        for caps in [WINDOWS, GTK] {
            for section in english(caps) {
                assert!(!matches!(section.entries.first(), Some(Entry::Separator)));
                assert!(!matches!(section.entries.last(), Some(Entry::Separator)));
                for pair in section.entries.windows(2) {
                    assert!(
                        !(pair[0] == Entry::Separator && pair[1] == Entry::Separator),
                        "{}",
                        section.id
                    );
                }
            }
        }
    }

    #[test]
    fn the_labels_are_the_uis_words() {
        let labels = Labels {
            menu_file: "&Datei".to_owned(),
            menu_print: "&Drucken\u{2026}".to_owned(),
            menu_copy: "&Kopieren".to_owned(),
            ..Labels::default()
        };
        let sections = describe(&labels, WINDOWS);
        assert_eq!(section(&sections, "menu-file").label, "&Datei");
        let all = labels_of(&sections);
        assert!(all.contains(&"&Drucken\u{2026}".to_owned()));
        assert!(all.contains(&"&Kopieren".to_owned()));
        // The English ones are untouched.
        assert!(all.contains(&"&Quit".to_owned()));
    }

    #[test]
    fn labels_come_in_the_order_the_handles_are_kept() {
        // One per menu, then one per entry that is not a separator.
        let sections = english(WINDOWS);
        let expected: usize = sections
            .iter()
            .map(|section| {
                1 + section
                    .entries
                    .iter()
                    .filter(|entry| **entry != Entry::Separator)
                    .count()
            })
            .sum();
        assert_eq!(labels_of(&sections).len(), expected);
        assert_eq!(shown_of(&sections).len(), expected);
        assert_eq!(labels_of(&sections)[0], "&File");
        assert_eq!(labels_of(&sections)[1], "&New page");
    }

    // ----- hints -----------------------------------------------------------------------------------------------

    #[test]
    fn a_hint_is_text_after_a_tab_in_the_label_where_the_platform_draws_one() {
        let sections = english(WINDOWS);
        let file = section(&sections, "menu-file");
        let print = file
            .entries
            .iter()
            .find(|entry| {
                matches!(
                    entry,
                    Entry::Item {
                        item: Item::Print,
                        ..
                    }
                )
            })
            .unwrap();
        assert_eq!(print.shown().unwrap(), "&Print\u{2026}\tCtrl+P");
        // The label alone is what `text()` answers, so a label change keeps the hint and a test reads the label.
        assert_eq!(print.label().unwrap(), "&Print\u{2026}");
        let shown = shown_of(&sections);
        for text in [
            "&Find in page\tCtrl+F",
            "Toggle &sidebar\tCtrl+\\",
            "&Back\tAlt+Left",
            "&Forward\tAlt+Right",
            "&Close window\tAlt+F4",
        ] {
            assert!(shown.contains(&text.to_owned()), "{text}");
        }
    }

    #[test]
    fn items_without_a_shortcut_in_the_ui_show_none() {
        let shown = shown_of(&english(WINDOWS));
        for item in [
            "&New page",
            "&Import\u{2026}",
            "&Export\u{2026}",
            "&Lock",
            "&Quit",
            "&Minimize",
            "&About atomik-kb",
        ] {
            assert!(shown.contains(&item.to_owned()), "{item}: {shown:?}");
        }
    }

    #[test]
    fn no_hint_where_the_platform_would_show_the_tab_itself() {
        for section in english(GTK) {
            for entry in &section.entries {
                if let Entry::Item { hint, .. } = entry {
                    assert_eq!(*hint, None);
                }
                if let Some(shown) = entry.shown() {
                    assert!(!shown.contains('\t'), "{shown:?}");
                }
            }
        }
    }

    #[test]
    fn a_hint_is_never_an_accelerator() {
        // The menu is made from `MenuItem::with_id(.., None)`: the description has no accelerator field at
        // all, so nothing can be registered with the window's accelerator table. The hint is part of the text.
        let sections = english(WINDOWS);
        let hinted = sections
            .iter()
            .flat_map(|section| &section.entries)
            .filter(|entry| matches!(entry, Entry::Item { hint: Some(_), .. }))
            .count();
        assert_eq!(hinted, 6);
        for item in Item::ALL {
            if let Some(hint) = item.hint() {
                assert!(!hint.contains('\t'), "{item:?}");
                assert!(!hint.is_empty(), "{item:?}");
            }
        }
    }

    #[test]
    fn the_hints_are_not_the_global_capture_shortcut() {
        // Ctrl+Alt+N is the quick capture's and the user can change it: no item shows it.
        for item in Item::ALL {
            assert_ne!(item.hint(), Some("Ctrl+Alt+N"), "{item:?}");
        }
    }

    // ----- mnemonics -------------------------------------------------------------------------------------------

    #[test]
    fn the_english_mnemonics_are_unique_in_each_menu_and_across_the_bar() {
        for caps in [WINDOWS, GTK] {
            let sections = english(caps);
            let bar: Vec<char> = sections
                .iter()
                .map(|section| mnemonic(&section.label).unwrap())
                .collect();
            assert_eq!(
                bar.iter().collect::<HashSet<_>>().len(),
                bar.len(),
                "{bar:?}"
            );
            for section in &sections {
                let letters: Vec<char> = section
                    .entries
                    .iter()
                    .filter_map(Entry::label)
                    .map(|label| mnemonic(label).unwrap_or(' '))
                    .collect();
                assert!(!letters.contains(&' '), "{}: a label has none", section.id);
                assert_eq!(
                    letters.iter().collect::<HashSet<_>>().len(),
                    letters.len(),
                    "{}: {letters:?}",
                    section.id
                );
            }
        }
    }

    #[test]
    fn a_double_ampersand_is_a_literal_one() {
        assert_eq!(mnemonic("&File"), Some('f'));
        assert_eq!(mnemonic("Cu&t"), Some('t'));
        assert_eq!(mnemonic("Q&&A"), None);
        assert_eq!(mnemonic("Q&&A &Print"), Some('p'));
        assert_eq!(mnemonic("Open"), None);
    }

    // ----- the hints are the UI's real shortcuts ---------------------------------------------------------------

    #[test]
    fn the_hint_constants_are_what_the_ui_binds() {
        assert_eq!(HINT_PRINT, "Ctrl+P");
        assert_eq!(HINT_FIND, "Ctrl+F");
        assert_eq!(HINT_TOGGLE_SIDEBAR, "Ctrl+\\");
        assert_eq!(HINT_BACK, "Alt+Left");
        assert_eq!(HINT_FORWARD, "Alt+Right");
        assert_eq!(HINT_CLOSE_WINDOW, "Alt+F4");
    }
}
