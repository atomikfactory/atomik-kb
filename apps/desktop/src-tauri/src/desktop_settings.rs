//! The two device settings of the desktop shell's tray and quick capture (ADR-0040), read the way the shell
//! needs them. The core stores any JSON under a `namespace.name` key (`SettingsGet`, `SettingsSet`) and does
//! not know these two; the Settings screen writes them and the shell reads them (`desktop`).
//!
//! | key                      | value                                      | when absent |
//! |--------------------------|--------------------------------------------|-------------|
//! | `app.tray.close_to_tray` | `true`: the close button hides the window to the tray, `false`: it exits | `true` |
//! | `app.shortcut.capture`   | an accelerator (`Ctrl+Alt+N`); `""`: no shortcut | `Ctrl+Alt+N` |
//!
//! Both readers are pure and total: a value of another type than the one above is what an absent one is
//! (`app.tray.close_to_tray`) or an invalid shortcut (`app.shortcut.capture`, which then registers nothing),
//! never a panic and never an error.

use kb_types::Json;

use crate::accelerator::Accelerator;

/// The device setting that decides what the window's close button does.
pub const CLOSE_TO_TRAY_SETTING: &str = "app.tray.close_to_tray";

/// The device setting that holds the quick-capture accelerator.
pub const CAPTURE_SHORTCUT_SETTING: &str = "app.shortcut.capture";

/// The accelerator used while `app.shortcut.capture` is absent (or `null`).
pub const DEFAULT_CAPTURE_SHORTCUT: &str = "Ctrl+Alt+N";

/// What the stored `app.shortcut.capture` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureShortcut {
    /// Nothing is registered: the setting is the empty text (or only spaces).
    Disabled,
    /// This accelerator is to be registered.
    Enabled(Accelerator),
    /// The setting holds something that is not an accelerator of this shell; nothing is registered.
    Invalid,
}

/// Whether closing the window hides it to the tray: `true` unless the setting is the boolean `false`.
#[must_use]
pub fn close_to_tray(value: Option<&Json>) -> bool {
    !matches!(value, Some(Json::Bool(false)))
}

/// What `app.shortcut.capture` asks for: the default when it is absent or `null`, no shortcut for the empty
/// text, the accelerator for a valid one, and [`CaptureShortcut::Invalid`] for anything else.
#[must_use]
pub fn capture_shortcut(value: Option<&Json>) -> CaptureShortcut {
    match value {
        None | Some(Json::Null) => parse(DEFAULT_CAPTURE_SHORTCUT),
        Some(Json::String(text)) if text.trim().is_empty() => CaptureShortcut::Disabled,
        Some(Json::String(text)) => parse(text),
        Some(_) => CaptureShortcut::Invalid,
    }
}

fn parse(text: &str) -> CaptureShortcut {
    Accelerator::parse(text).map_or(CaptureShortcut::Invalid, CaptureShortcut::Enabled)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn enabled(text: &str) -> CaptureShortcut {
        CaptureShortcut::Enabled(Accelerator::parse(text).unwrap())
    }

    #[test]
    fn the_keys_are_the_ones_the_settings_screen_writes() {
        assert_eq!(CLOSE_TO_TRAY_SETTING, "app.tray.close_to_tray");
        assert_eq!(CAPTURE_SHORTCUT_SETTING, "app.shortcut.capture");
        assert_eq!(DEFAULT_CAPTURE_SHORTCUT, "Ctrl+Alt+N");
    }

    #[test]
    fn closing_hides_to_the_tray_unless_the_setting_says_false() {
        assert!(close_to_tray(None));
        assert!(close_to_tray(Some(&json!(true))));
        assert!(!close_to_tray(Some(&json!(false))));
        // Anything that is not the boolean false is as good as nothing.
        for other in [
            json!(null),
            json!(0),
            json!(1),
            json!("false"),
            json!("no"),
            json!([]),
            json!({ "value": false }),
        ] {
            assert!(close_to_tray(Some(&other)), "{other}");
        }
    }

    #[test]
    fn an_absent_shortcut_setting_is_the_default() {
        assert_eq!(capture_shortcut(None), enabled(DEFAULT_CAPTURE_SHORTCUT));
        assert_eq!(
            capture_shortcut(Some(&json!(null))),
            enabled(DEFAULT_CAPTURE_SHORTCUT)
        );
        assert!(Accelerator::parse(DEFAULT_CAPTURE_SHORTCUT).is_ok());
    }

    #[test]
    fn the_empty_text_turns_the_shortcut_off() {
        for text in ["", " ", "\t\n"] {
            assert_eq!(
                capture_shortcut(Some(&json!(text))),
                CaptureShortcut::Disabled,
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_valid_accelerator_is_read_in_its_canonical_form() {
        assert_eq!(
            capture_shortcut(Some(&json!("ctrl+shift+k"))),
            enabled("Ctrl+Shift+K")
        );
        assert_eq!(
            capture_shortcut(Some(&json!("Alt+Space"))),
            enabled("Alt+Space")
        );
    }

    #[test]
    fn what_is_not_an_accelerator_registers_nothing() {
        for value in [
            json!("N"),
            json!("Shift+N"),
            json!("Ctrl+"),
            json!("Ctrl+Nope"),
            json!("Ctrl+Alt+N+M"),
            json!(5),
            json!(true),
            json!(["Ctrl+Alt+N"]),
            json!({ "accelerator": "Ctrl+Alt+N" }),
            json!("Ctrl+Alt+N".repeat(20)),
        ] {
            assert_eq!(
                capture_shortcut(Some(&value)),
                CaptureShortcut::Invalid,
                "{value}"
            );
        }
    }
}
