//! The accelerator of the quick-capture shortcut (ADR-0040): the text the device setting
//! `app.shortcut.capture` holds, read strictly and written in one canonical form.
//!
//! The shell registers the shortcut with `tauri-plugin-global-shortcut`, whose own parser is lenient (it
//! accepts a bare key, the numeric keypad, media keys, any case) and answers a failure with text that
//! carries the string it was given. [`Accelerator::parse`] is pure, runs first, and accepts only what the
//! Settings recorder can produce and what is safe to take away from every other program:
//!
//! - **Modifiers** `Ctrl` (or `Control`), `Alt` (or `Option`), `Shift`, `Super` (or `Meta`, `Win`, `Windows`,
//!   `Cmd`, `Command`) and `CommandOrControl` (`CmdOrCtrl`, `CommandOrCtrl`, `CmdOrControl`), which is `Super`
//!   on macOS and `Ctrl` elsewhere. In any case, each at most once.
//! - **At least one of Ctrl, Alt or Super.** `Shift` alone is not a modifier here: `Shift+A` would take every
//!   capital A away from every other program, and a bare key would take the key.
//! - **Exactly one key, last:** a letter, a digit, `F1` to `F24`, `Space`, `Enter`, `Tab`, `Backspace`,
//!   `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, the arrows (`ArrowUp` or `Up`, ...), `Escape`
//!   and the punctuation keys of the main block (`Backquote`, `Backslash`, `BracketLeft`, `BracketRight`,
//!   `Comma`, `Equal`, `Minus`, `Period`, `Quote`, `Semicolon`, `Slash`, or the character itself except `+`,
//!   which is the separator).
//! - At most [`MAX_LEN`] bytes, parts separated by `+` with optional spaces around them.
//!
//! [`Accelerator::normalised`] writes the canonical form, `Ctrl+Alt+Shift+Super+Key` with the spellings above
//! (`Ctrl+Alt+N`), which `tauri-plugin-global-shortcut` reads back to the same shortcut (a test checks every key
//! against its parser).

/// The longest accelerator text read, in bytes. The longest canonical one is 33.
pub const MAX_LEN: usize = 64;

/// Why a text is not an accelerator of this shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceleratorError {
    /// Nothing but whitespace.
    Empty,
    /// More than [`MAX_LEN`] bytes.
    TooLong,
    /// An empty part (`Ctrl++N`, a leading or trailing `+`).
    Malformed,
    /// A part before the last that is not a modifier name.
    UnknownModifier,
    /// A modifier named twice (`CommandOrControl+Ctrl+N` counts: on Windows and Linux they are the same one).
    DuplicateModifier,
    /// Only modifiers.
    MissingKey,
    /// The last part is not a key of the list above.
    UnsupportedKey,
    /// No Ctrl, Alt or Super: the combination would take a plain key (or Shift and a key) from every program.
    NeedsModifier,
}

/// A valid accelerator: modifiers and one key, in canonical spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accelerator {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
    key: String,
}

impl Accelerator {
    /// Reads `text` on this platform (`CommandOrControl` is `Super` on macOS, `Ctrl` elsewhere).
    ///
    /// # Errors
    ///
    /// Why `text` is not an accelerator (see [`AcceleratorError`]).
    pub fn parse(text: &str) -> Result<Self, AcceleratorError> {
        Self::parse_for(text, cfg!(target_os = "macos"))
    }

    /// Reads `text` for a platform where `CommandOrControl` is `Super` (`mac`) or `Ctrl` (anywhere else).
    ///
    /// # Errors
    ///
    /// Why `text` is not an accelerator (see [`AcceleratorError`]).
    pub fn parse_for(text: &str, mac: bool) -> Result<Self, AcceleratorError> {
        if text.trim().is_empty() {
            return Err(AcceleratorError::Empty);
        }
        if text.len() > MAX_LEN {
            return Err(AcceleratorError::TooLong);
        }
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(AcceleratorError::Malformed);
        }
        let Some((last, modifiers)) = parts.split_last() else {
            return Err(AcceleratorError::Empty);
        };
        let mut found = Accelerator {
            ctrl: false,
            alt: false,
            shift: false,
            logo: false,
            key: String::new(),
        };
        for part in modifiers {
            found.add_modifier(part, mac)?;
        }
        found.key = match canonical_key(last) {
            Some(key) => key,
            // `Ctrl+Alt` ends in a modifier: a key is missing, which is not the same as a key we do not know.
            None if modifier_of(last, mac).is_some() => return Err(AcceleratorError::MissingKey),
            None => return Err(AcceleratorError::UnsupportedKey),
        };
        if !(found.ctrl || found.alt || found.logo) {
            return Err(AcceleratorError::NeedsModifier);
        }
        Ok(found)
    }

    fn add_modifier(&mut self, name: &str, mac: bool) -> Result<(), AcceleratorError> {
        let slot = match modifier_of(name, mac) {
            Some(Modifier::Ctrl) => &mut self.ctrl,
            Some(Modifier::Alt) => &mut self.alt,
            Some(Modifier::Shift) => &mut self.shift,
            Some(Modifier::Logo) => &mut self.logo,
            None => return Err(AcceleratorError::UnknownModifier),
        };
        if *slot {
            return Err(AcceleratorError::DuplicateModifier);
        }
        *slot = true;
        Ok(())
    }

    /// The canonical text: `Ctrl`, `Alt`, `Shift`, `Super` (those that are set, in this order) and the key,
    /// joined by `+`.
    #[must_use]
    pub fn normalised(&self) -> String {
        let mut text = String::new();
        for (set, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.logo, "Super"),
        ] {
            if set {
                text.push_str(name);
                text.push('+');
            }
        }
        text.push_str(&self.key);
        text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Logo,
}

/// The modifier a part names, in any case.
fn modifier_of(name: &str, mac: bool) -> Option<Modifier> {
    match name.to_ascii_uppercase().as_str() {
        "CTRL" | "CONTROL" => Some(Modifier::Ctrl),
        "ALT" | "OPTION" => Some(Modifier::Alt),
        "SHIFT" => Some(Modifier::Shift),
        "SUPER" | "META" | "WIN" | "WINDOWS" | "CMD" | "COMMAND" => Some(Modifier::Logo),
        "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL" | "CMDORCONTROL" => {
            Some(if mac { Modifier::Logo } else { Modifier::Ctrl })
        }
        _ => None,
    }
}

/// The canonical spelling of the key a part names, in any case (`None` for what is not a supported key).
fn canonical_key(name: &str) -> Option<String> {
    let upper = name.to_ascii_uppercase();
    // One letter or digit.
    if let [byte] = upper.as_bytes()
        && byte.is_ascii_alphanumeric()
    {
        return Some(upper);
    }
    // `F1` to `F24`, without a leading zero.
    if let Some(digits) = upper.strip_prefix('F')
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && digits
            .parse::<u8>()
            .is_ok_and(|number| (1..=24).contains(&number))
    {
        return Some(format!("F{digits}"));
    }
    let canonical = match upper.as_str() {
        "SPACE" => "Space",
        "ENTER" => "Enter",
        "TAB" => "Tab",
        "BACKSPACE" => "Backspace",
        "DELETE" => "Delete",
        "INSERT" => "Insert",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" => "PageUp",
        "PAGEDOWN" => "PageDown",
        "ARROWUP" | "UP" => "ArrowUp",
        "ARROWDOWN" | "DOWN" => "ArrowDown",
        "ARROWLEFT" | "LEFT" => "ArrowLeft",
        "ARROWRIGHT" | "RIGHT" => "ArrowRight",
        "ESCAPE" | "ESC" => "Escape",
        "BACKQUOTE" | "`" => "Backquote",
        "BACKSLASH" | "\\" => "Backslash",
        "BRACKETLEFT" | "[" => "BracketLeft",
        "BRACKETRIGHT" | "]" => "BracketRight",
        "COMMA" | "," => "Comma",
        "EQUAL" | "=" => "Equal",
        "MINUS" | "-" => "Minus",
        "PERIOD" | "." => "Period",
        "QUOTE" | "'" => "Quote",
        "SEMICOLON" | ";" => "Semicolon",
        "SLASH" | "/" => "Slash",
        _ => return None,
    };
    Some(canonical.to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::str::FromStr;

    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

    use super::*;

    fn parsed(text: &str) -> Accelerator {
        Accelerator::parse_for(text, false).unwrap()
    }

    fn refused(text: &str) -> AcceleratorError {
        Accelerator::parse_for(text, false).unwrap_err()
    }

    #[test]
    fn the_default_is_an_accelerator_and_is_already_canonical() {
        assert_eq!(parsed("Ctrl+Alt+N").normalised(), "Ctrl+Alt+N");
    }

    #[test]
    fn modifiers_and_keys_are_read_in_any_case_and_written_canonically() {
        for (text, canonical) in [
            ("ctrl+alt+n", "Ctrl+Alt+N"),
            ("CONTROL+SHIFT+k", "Ctrl+Shift+K"),
            ("Alt+Ctrl+9", "Ctrl+Alt+9"),
            ("Shift+Alt+Super+Ctrl+F12", "Ctrl+Alt+Shift+Super+F12"),
            ("Option+space", "Alt+Space"),
            ("Cmd+Shift+Enter", "Shift+Super+Enter"),
            ("Win+e", "Super+E"),
            ("Meta+Tab", "Super+Tab"),
            ("ctrl+up", "Ctrl+ArrowUp"),
            ("Ctrl+arrowleft", "Ctrl+ArrowLeft"),
            ("Ctrl+Esc", "Ctrl+Escape"),
            ("Ctrl+PAGEDOWN", "Ctrl+PageDown"),
            ("Ctrl+Backspace", "Ctrl+Backspace"),
        ] {
            assert_eq!(parsed(text).normalised(), canonical, "{text}");
        }
    }

    #[test]
    fn spaces_around_the_parts_are_fine_and_nothing_else_is() {
        assert_eq!(parsed("  Ctrl + Alt + N ").normalised(), "Ctrl+Alt+N");
        // Whitespace at the ends is trimmed like the spaces around a part.
        assert_eq!(parsed("Ctrl+Alt+N\n").normalised(), "Ctrl+Alt+N");
        assert_eq!(refused("Ctrl+A lt+N"), AcceleratorError::UnknownModifier);
        assert_eq!(refused("Ctrl+Alt+N M"), AcceleratorError::UnsupportedKey);
    }

    #[test]
    fn command_or_control_is_ctrl_except_on_macos() {
        for spelling in [
            "CommandOrControl",
            "CmdOrCtrl",
            "CommandOrCtrl",
            "cmdorcontrol",
        ] {
            let text = format!("{spelling}+Shift+K");
            assert_eq!(
                Accelerator::parse_for(&text, false).unwrap().normalised(),
                "Ctrl+Shift+K",
                "{text}"
            );
            assert_eq!(
                Accelerator::parse_for(&text, true).unwrap().normalised(),
                "Shift+Super+K",
                "{text}"
            );
        }
        // It is the same modifier as the one it stands for.
        assert_eq!(
            Accelerator::parse_for("CmdOrCtrl+Ctrl+K", false).unwrap_err(),
            AcceleratorError::DuplicateModifier
        );
        assert_eq!(
            Accelerator::parse_for("CmdOrCtrl+Super+K", true).unwrap_err(),
            AcceleratorError::DuplicateModifier
        );
        assert!(Accelerator::parse_for("CmdOrCtrl+Super+K", false).is_ok());
    }

    #[test]
    fn letters_digits_function_keys_and_named_keys_are_keys() {
        for letter in 'A'..='Z' {
            assert_eq!(
                parsed(&format!("Ctrl+{letter}")).normalised(),
                format!("Ctrl+{letter}")
            );
            assert_eq!(
                parsed(&format!("Ctrl+{}", letter.to_ascii_lowercase())).normalised(),
                format!("Ctrl+{letter}")
            );
        }
        for digit in 0..=9 {
            assert_eq!(
                parsed(&format!("Alt+{digit}")).normalised(),
                format!("Alt+{digit}")
            );
        }
        for number in 1..=24 {
            assert_eq!(
                parsed(&format!("Alt+f{number}")).normalised(),
                format!("Alt+F{number}")
            );
        }
    }

    #[test]
    fn the_punctuation_keys_are_read_by_name_and_by_character() {
        for (names, canonical) in [
            (["Backquote", "`"], "Backquote"),
            (["Backslash", "\\"], "Backslash"),
            (["BracketLeft", "["], "BracketLeft"),
            (["BracketRight", "]"], "BracketRight"),
            (["Comma", ","], "Comma"),
            (["Equal", "="], "Equal"),
            (["Minus", "-"], "Minus"),
            (["Period", "."], "Period"),
            (["Quote", "'"], "Quote"),
            (["Semicolon", ";"], "Semicolon"),
            (["Slash", "/"], "Slash"),
        ] {
            for name in names {
                assert_eq!(
                    parsed(&format!("Ctrl+{name}")).normalised(),
                    format!("Ctrl+{canonical}"),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn a_combination_without_ctrl_alt_or_super_is_refused() {
        for text in [
            "N",
            "F5",
            "Space",
            "Shift+N",
            "Shift+F5",
            "Shift+Space",
            "Escape",
        ] {
            assert_eq!(refused(text), AcceleratorError::NeedsModifier, "{text}");
        }
        // Each of the three is enough, alone.
        for text in ["Ctrl+N", "Alt+N", "Super+N", "Shift+Alt+N"] {
            assert!(Accelerator::parse_for(text, false).is_ok(), "{text}");
        }
    }

    #[test]
    fn a_text_that_is_not_one_combination_is_refused() {
        for (text, error) in [
            ("", AcceleratorError::Empty),
            ("   ", AcceleratorError::Empty),
            ("Ctrl+", AcceleratorError::Malformed),
            ("+N", AcceleratorError::Malformed),
            ("Ctrl++N", AcceleratorError::Malformed),
            ("Ctrl+Alt+", AcceleratorError::Malformed),
            ("+", AcceleratorError::Malformed),
            ("Ctrl+Alt", AcceleratorError::MissingKey),
            ("Ctrl", AcceleratorError::MissingKey),
            ("Ctrl+Shift+Super", AcceleratorError::MissingKey),
            ("Ctrl+Ctrl+N", AcceleratorError::DuplicateModifier),
            ("Ctrl+Control+N", AcceleratorError::DuplicateModifier),
            ("Alt+Option+N", AcceleratorError::DuplicateModifier),
            ("Ctrl+N+Alt", AcceleratorError::UnknownModifier),
            ("Ctrl+N+M", AcceleratorError::UnknownModifier),
            ("Hyper+N", AcceleratorError::UnknownModifier),
            ("Ctrl+Nope", AcceleratorError::UnsupportedKey),
            ("Ctrl+F0", AcceleratorError::UnsupportedKey),
            ("Ctrl+F25", AcceleratorError::UnsupportedKey),
            ("Ctrl+F05", AcceleratorError::UnsupportedKey),
            ("Ctrl+F+1", AcceleratorError::UnknownModifier),
            ("Ctrl+Numpad1", AcceleratorError::UnsupportedKey),
            ("Ctrl+MediaPlay", AcceleratorError::UnsupportedKey),
            ("Ctrl+NumLock", AcceleratorError::UnsupportedKey),
            ("Ctrl+CapsLock", AcceleratorError::UnsupportedKey),
            ("Ctrl+10", AcceleratorError::UnsupportedKey),
            ("Ctrl+\u{e9}", AcceleratorError::UnsupportedKey),
            ("Ctrl+\u{1f600}", AcceleratorError::UnsupportedKey),
        ] {
            assert_eq!(refused(text), error, "{text:?}");
        }
    }

    #[test]
    fn a_long_text_is_refused_without_being_read() {
        assert_eq!(refused(&"Ctrl+".repeat(100)), AcceleratorError::TooLong);
        assert_eq!(refused(&"a".repeat(MAX_LEN + 1)), AcceleratorError::TooLong);
        let at_cap = format!("Ctrl+{}", " ".repeat(MAX_LEN - 6)) + "N";
        assert!(at_cap.len() <= MAX_LEN);
        assert!(Accelerator::parse_for(&at_cap, false).is_ok());
        let longest = "Ctrl+Alt+Shift+Super+BracketRight";
        assert_eq!(longest.len(), 33);
        assert!(longest.len() <= MAX_LEN);
        assert_eq!(parsed(longest).normalised(), longest);
    }

    #[test]
    fn every_key_is_one_the_plugin_reads_back_as_the_same_shortcut() {
        let mut keys: Vec<String> = Vec::new();
        keys.extend(('A'..='Z').map(String::from));
        keys.extend((0..=9).map(|digit| digit.to_string()));
        keys.extend((1..=24).map(|number| format!("F{number}")));
        keys.extend(
            [
                "Space",
                "Enter",
                "Tab",
                "Backspace",
                "Delete",
                "Insert",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "ArrowUp",
                "ArrowDown",
                "ArrowLeft",
                "ArrowRight",
                "Escape",
                "Backquote",
                "Backslash",
                "BracketLeft",
                "BracketRight",
                "Comma",
                "Equal",
                "Minus",
                "Period",
                "Quote",
                "Semicolon",
                "Slash",
            ]
            .map(str::to_owned),
        );
        for key in keys {
            let text = format!("Ctrl+Alt+Shift+Super+{key}");
            let accelerator = parsed(&text);
            assert_eq!(accelerator.normalised(), text);
            let read = Shortcut::from_str(&accelerator.normalised());
            assert!(read.is_ok(), "{text}: {read:?}");
            let shortcut = read.unwrap();
            assert_eq!(
                shortcut.mods,
                Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::SUPER,
                "{text}"
            );
            assert_ne!(shortcut.key, Code::Unidentified, "{text}");
        }
    }

    #[test]
    fn the_default_registers_as_control_alt_n() {
        let shortcut = Shortcut::from_str(&parsed("Ctrl+Alt+N").normalised()).unwrap();
        assert_eq!(shortcut.mods, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(shortcut.key, Code::KeyN);
    }
}
