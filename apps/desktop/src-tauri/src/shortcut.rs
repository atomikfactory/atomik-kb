//! The quick-capture shortcut (ADR-0040): which accelerator is registered with the operating system, and the
//! state the Settings screen reports.
//!
//! `tauri-plugin-global-shortcut` is registered on the builder by [`crate::run`] with one handler for every
//! shortcut it knows ([`is_press`]: a press is a capture, a release is nothing). The webview is granted none of
//! its permissions, so only this module registers or removes a shortcut. [`Controller`] is the logic around
//! it, behind the small [`Registrar`] seam so that the tests can run it without an operating system:
//!
//! - one shortcut at a time: a new accelerator removes the old one first, and if the operating system refuses
//!   it (another program owns it) nothing stays registered and the state is [`ShortcutState::Taken`];
//! - an unchanged accelerator is not registered again, except after a refusal (the other program may have let
//!   go of it);
//! - an empty setting or an invalid one removes the shortcut ([`ShortcutState::Disabled`], [`Invalid`]).
//!
//! The state goes to the webview as a shell event (`shell_events`), sticky, so a Settings screen opened later
//! still shows it. Nothing here logs an accelerator or the operating system's error text: the state only.
//!
//! [`Invalid`]: ShortcutState::Invalid

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcut, ShortcutEvent, ShortcutState as KeyState};

use crate::accelerator::Accelerator;
use crate::desktop_settings::CaptureShortcut;

/// What the Settings screen shows about the quick-capture shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShortcutState {
    /// The setting is empty: no shortcut.
    Disabled,
    /// The shortcut is registered and works.
    Registered,
    /// The operating system refused the shortcut: another program uses it, or it is not allowed.
    Taken,
    /// The setting is not an accelerator of this shell; nothing is registered.
    Invalid,
}

/// Registers and removes one accelerator with the operating system.
pub trait Registrar {
    /// Registers `accelerator`; `false` when the operating system refuses it.
    fn register(&mut self, accelerator: &Accelerator) -> bool;
    /// Removes a shortcut this registrar registered (a failure to remove it is its own to log).
    fn unregister(&mut self, accelerator: &Accelerator);
}

/// Keeps the one registered shortcut in step with the setting.
pub struct Controller<G: Registrar> {
    registrar: G,
    /// The accelerator that is registered now.
    active: Option<Accelerator>,
}

impl<G: Registrar> Controller<G> {
    /// Nothing is registered.
    pub fn new(registrar: G) -> Self {
        Self {
            registrar,
            active: None,
        }
    }

    /// The accelerator that is registered now, if any.
    #[must_use]
    pub fn active(&self) -> Option<&Accelerator> {
        self.active.as_ref()
    }

    /// Makes what is registered match `wanted` and answers the state.
    pub fn apply(&mut self, wanted: &CaptureShortcut) -> ShortcutState {
        match wanted {
            CaptureShortcut::Disabled => {
                self.remove();
                ShortcutState::Disabled
            }
            CaptureShortcut::Invalid => {
                self.remove();
                ShortcutState::Invalid
            }
            CaptureShortcut::Enabled(accelerator) => {
                if self.active.as_ref() == Some(accelerator) {
                    return ShortcutState::Registered;
                }
                self.remove();
                if self.registrar.register(accelerator) {
                    self.active = Some(accelerator.clone());
                    ShortcutState::Registered
                } else {
                    ShortcutState::Taken
                }
            }
        }
    }

    fn remove(&mut self) {
        if let Some(active) = self.active.take() {
            self.registrar.unregister(&active);
        }
    }
}

/// The real registrar: `tauri-plugin-global-shortcut`, which runs each call on the main thread and waits for
/// it. Without the plugin (the mock runtime of the tests) every registration is refused.
pub struct PluginRegistrar<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginRegistrar<R> {
    /// A registrar that talks to the plugin through `app`.
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> Registrar for PluginRegistrar<R> {
    fn register(&mut self, accelerator: &Accelerator) -> bool {
        let Some(shortcuts) = self.app.try_state::<GlobalShortcut<R>>() else {
            tracing::warn!("the global shortcut plugin is not available");
            return false;
        };
        match shortcuts.register(accelerator.normalised().as_str()) {
            Ok(()) => true,
            Err(_) => {
                // The error text names the accelerator: the state is all that is kept.
                tracing::warn!("the operating system refused the capture shortcut");
                false
            }
        }
    }

    fn unregister(&mut self, accelerator: &Accelerator) {
        let Some(shortcuts) = self.app.try_state::<GlobalShortcut<R>>() else {
            return;
        };
        if shortcuts
            .unregister(accelerator.normalised().as_str())
            .is_err()
        {
            tracing::warn!("the capture shortcut could not be removed");
        }
    }
}

/// Whether a key event of the plugin is the press that makes a capture (the release that follows is not).
#[must_use]
pub fn is_press(event: &ShortcutEvent) -> bool {
    event.state == KeyState::Pressed
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Register(String),
        Unregister(String),
    }

    /// Records the calls; refuses the accelerators in `taken`.
    #[derive(Default)]
    struct Fake {
        calls: Rc<RefCell<Vec<Call>>>,
        taken: Rc<RefCell<Vec<String>>>,
    }

    impl Registrar for Fake {
        fn register(&mut self, accelerator: &Accelerator) -> bool {
            let text = accelerator.normalised();
            self.calls.borrow_mut().push(Call::Register(text.clone()));
            !self.taken.borrow().contains(&text)
        }

        fn unregister(&mut self, accelerator: &Accelerator) {
            self.calls
                .borrow_mut()
                .push(Call::Unregister(accelerator.normalised()));
        }
    }

    type Calls = Rc<RefCell<Vec<Call>>>;
    type Taken = Rc<RefCell<Vec<String>>>;

    fn controller() -> (Controller<Fake>, Calls, Taken) {
        let fake = Fake::default();
        let calls = Rc::clone(&fake.calls);
        let taken = Rc::clone(&fake.taken);
        (Controller::new(fake), calls, taken)
    }

    fn wanted(text: &str) -> CaptureShortcut {
        CaptureShortcut::Enabled(Accelerator::parse(text).unwrap())
    }

    fn register(text: &str) -> Call {
        Call::Register(text.to_owned())
    }

    fn unregister(text: &str) -> Call {
        Call::Unregister(text.to_owned())
    }

    #[test]
    fn the_state_serialises_as_the_webview_reads_it() {
        for (state, text) in [
            (ShortcutState::Disabled, "disabled"),
            (ShortcutState::Registered, "registered"),
            (ShortcutState::Taken, "taken"),
            (ShortcutState::Invalid, "invalid"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), text);
        }
    }

    #[test]
    fn an_accelerator_is_registered_once() {
        let (mut controller, calls, _) = controller();
        assert_eq!(
            controller.apply(&wanted("Ctrl+Alt+N")),
            ShortcutState::Registered
        );
        assert_eq!(
            controller.apply(&wanted("ctrl+alt+n")),
            ShortcutState::Registered
        );
        assert_eq!(*calls.borrow(), vec![register("Ctrl+Alt+N")]);
        assert_eq!(
            controller.active().map(Accelerator::normalised),
            Some("Ctrl+Alt+N".to_owned())
        );
    }

    #[test]
    fn a_new_accelerator_replaces_the_old_one() {
        let (mut controller, calls, _) = controller();
        controller.apply(&wanted("Ctrl+Alt+N"));
        assert_eq!(
            controller.apply(&wanted("Ctrl+Shift+K")),
            ShortcutState::Registered
        );
        assert_eq!(
            *calls.borrow(),
            vec![
                register("Ctrl+Alt+N"),
                unregister("Ctrl+Alt+N"),
                register("Ctrl+Shift+K")
            ]
        );
    }

    #[test]
    fn an_empty_setting_removes_the_shortcut() {
        let (mut controller, calls, _) = controller();
        controller.apply(&wanted("Ctrl+Alt+N"));
        assert_eq!(
            controller.apply(&CaptureShortcut::Disabled),
            ShortcutState::Disabled
        );
        assert_eq!(controller.active(), None);
        // Nothing left to remove the second time.
        assert_eq!(
            controller.apply(&CaptureShortcut::Disabled),
            ShortcutState::Disabled
        );
        assert_eq!(
            *calls.borrow(),
            vec![register("Ctrl+Alt+N"), unregister("Ctrl+Alt+N")]
        );
    }

    #[test]
    fn an_invalid_setting_removes_the_shortcut_and_says_so() {
        let (mut controller, calls, _) = controller();
        controller.apply(&wanted("Ctrl+Alt+N"));
        assert_eq!(
            controller.apply(&CaptureShortcut::Invalid),
            ShortcutState::Invalid
        );
        assert_eq!(controller.active(), None);
        assert_eq!(
            *calls.borrow(),
            vec![register("Ctrl+Alt+N"), unregister("Ctrl+Alt+N")]
        );
    }

    #[test]
    fn a_shortcut_another_program_owns_is_taken_and_nothing_stays_registered() {
        let (mut controller, calls, taken) = controller();
        taken.borrow_mut().push("Ctrl+Alt+N".to_owned());
        assert_eq!(
            controller.apply(&wanted("Ctrl+Alt+N")),
            ShortcutState::Taken
        );
        assert_eq!(controller.active(), None);
        assert_eq!(*calls.borrow(), vec![register("Ctrl+Alt+N")]);
    }

    #[test]
    fn a_refused_replacement_leaves_no_shortcut_behind() {
        let (mut controller, calls, taken) = controller();
        controller.apply(&wanted("Ctrl+Alt+N"));
        taken.borrow_mut().push("Ctrl+Shift+K".to_owned());
        assert_eq!(
            controller.apply(&wanted("Ctrl+Shift+K")),
            ShortcutState::Taken
        );
        assert_eq!(controller.active(), None);
        assert_eq!(
            *calls.borrow(),
            vec![
                register("Ctrl+Alt+N"),
                unregister("Ctrl+Alt+N"),
                register("Ctrl+Shift+K")
            ]
        );
    }

    #[test]
    fn a_refused_accelerator_is_tried_again_when_the_setting_is_read_again() {
        // The other program may have let go of it (the shell reads the setting again after an unlock).
        let (mut controller, calls, taken) = controller();
        taken.borrow_mut().push("Ctrl+Alt+N".to_owned());
        assert_eq!(
            controller.apply(&wanted("Ctrl+Alt+N")),
            ShortcutState::Taken
        );
        taken.borrow_mut().clear();
        assert_eq!(
            controller.apply(&wanted("Ctrl+Alt+N")),
            ShortcutState::Registered
        );
        assert_eq!(
            *calls.borrow(),
            vec![register("Ctrl+Alt+N"), register("Ctrl+Alt+N")]
        );
    }

    #[test]
    fn a_press_is_a_capture_and_a_release_is_not() {
        let press = ShortcutEvent {
            id: 1,
            state: KeyState::Pressed,
        };
        let release = ShortcutEvent {
            id: 1,
            state: KeyState::Released,
        };
        assert!(is_press(&press));
        assert!(!is_press(&release));
    }
}
