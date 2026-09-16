//! Keyboard shortcuts: parsing, display and conversion for the global hotkey and the settings UI.
//!
//! Shortcuts are written the way users see them, e.g. `Win+Shift+O`. `Win`, `Super`, `Meta` and
//! `Cmd` all mean the same key, so a config file works on every platform.

use std::fmt;
use std::str::FromStr;

use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Name of the Windows / Super / Command key on the current platform.
pub const SUPER_LABEL: &str = if cfg!(windows) {
    "Win"
} else if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Super"
};

/// Keys a shortcut can end with: (name, hotkey code, egui key).
#[rustfmt::skip]
const KEYS: &[(&str, Code, egui::Key)] = &[
    ("A", Code::KeyA, egui::Key::A), ("B", Code::KeyB, egui::Key::B), ("C", Code::KeyC, egui::Key::C),
    ("D", Code::KeyD, egui::Key::D), ("E", Code::KeyE, egui::Key::E), ("F", Code::KeyF, egui::Key::F),
    ("G", Code::KeyG, egui::Key::G), ("H", Code::KeyH, egui::Key::H), ("I", Code::KeyI, egui::Key::I),
    ("J", Code::KeyJ, egui::Key::J), ("K", Code::KeyK, egui::Key::K), ("L", Code::KeyL, egui::Key::L),
    ("M", Code::KeyM, egui::Key::M), ("N", Code::KeyN, egui::Key::N), ("O", Code::KeyO, egui::Key::O),
    ("P", Code::KeyP, egui::Key::P), ("Q", Code::KeyQ, egui::Key::Q), ("R", Code::KeyR, egui::Key::R),
    ("S", Code::KeyS, egui::Key::S), ("T", Code::KeyT, egui::Key::T), ("U", Code::KeyU, egui::Key::U),
    ("V", Code::KeyV, egui::Key::V), ("W", Code::KeyW, egui::Key::W), ("X", Code::KeyX, egui::Key::X),
    ("Y", Code::KeyY, egui::Key::Y), ("Z", Code::KeyZ, egui::Key::Z),
    ("0", Code::Digit0, egui::Key::Num0), ("1", Code::Digit1, egui::Key::Num1),
    ("2", Code::Digit2, egui::Key::Num2), ("3", Code::Digit3, egui::Key::Num3),
    ("4", Code::Digit4, egui::Key::Num4), ("5", Code::Digit5, egui::Key::Num5),
    ("6", Code::Digit6, egui::Key::Num6), ("7", Code::Digit7, egui::Key::Num7),
    ("8", Code::Digit8, egui::Key::Num8), ("9", Code::Digit9, egui::Key::Num9),
    ("F1", Code::F1, egui::Key::F1), ("F2", Code::F2, egui::Key::F2), ("F3", Code::F3, egui::Key::F3),
    ("F4", Code::F4, egui::Key::F4), ("F5", Code::F5, egui::Key::F5), ("F6", Code::F6, egui::Key::F6),
    ("F7", Code::F7, egui::Key::F7), ("F8", Code::F8, egui::Key::F8), ("F9", Code::F9, egui::Key::F9),
    ("F10", Code::F10, egui::Key::F10), ("F11", Code::F11, egui::Key::F11),
    ("F12", Code::F12, egui::Key::F12),
    ("Space", Code::Space, egui::Key::Space), ("Insert", Code::Insert, egui::Key::Insert),
    ("Delete", Code::Delete, egui::Key::Delete), ("Home", Code::Home, egui::Key::Home),
    ("End", Code::End, egui::Key::End), ("PageUp", Code::PageUp, egui::Key::PageUp),
    ("PageDown", Code::PageDown, egui::Key::PageDown), ("Up", Code::ArrowUp, egui::Key::ArrowUp),
    ("Down", Code::ArrowDown, egui::Key::ArrowDown), ("Left", Code::ArrowLeft, egui::Key::ArrowLeft),
    ("Right", Code::ArrowRight, egui::Key::ArrowRight),
];

/// A key combined with modifiers.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per modifier key, as shortcuts are written"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    ctrl: bool,
    alt: bool,
    shift: bool,
    /// Windows key on Windows, Super on Linux, Command on macOS.
    super_key: bool,
    /// Index into [`KEYS`].
    key: usize,
}

impl Default for Shortcut {
    /// `Win/Super+Shift+O`: follows the Snipping Tool pattern (Win+Shift+S) and is free in
    /// Windows, PowerToys, GNOME and KDE.
    fn default() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: true,
            super_key: true,
            key: key_index("O").unwrap_or(0),
        }
    }
}

impl Shortcut {
    /// Builds a shortcut from a key press in egui. egui does not report the Windows / Super key,
    /// so its state comes separately. Returns `None` for keys that cannot end a shortcut.
    pub fn from_key_press(pressed: egui::Key, modifiers: egui::Modifiers, super_key: bool) -> Option<Self> {
        let key = KEYS.iter().position(|&(_, _, key)| key == pressed)?;

        Some(Self {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            super_key,
            key,
        })
    }

    pub fn to_hotkey(self) -> HotKey {
        let mut modifiers = Modifiers::empty();
        modifiers.set(Modifiers::CONTROL, self.ctrl);
        modifiers.set(Modifiers::ALT, self.alt);
        modifiers.set(Modifiers::SHIFT, self.shift);
        modifiers.set(Modifiers::SUPER, self.super_key);

        HotKey::new(Some(modifiers), KEYS[self.key].1)
    }

    /// Why the shortcut cannot be used, if it cannot.
    pub fn problem(&self) -> Option<&'static str> {
        if !(self.ctrl || self.alt || self.shift || self.super_key) {
            return Some("Hold Ctrl, Alt or the Windows/Super key too, otherwise the key stops working elsewhere.");
        }

        if self.shift && !(self.ctrl || self.alt || self.super_key) {
            return Some("Shift alone would block typing; hold Ctrl, Alt or the Windows/Super key too.");
        }

        None
    }

    /// A caveat about a usable shortcut.
    pub fn warning(&self) -> Option<&'static str> {
        (cfg!(windows) && self.ctrl && self.alt && !self.super_key)
            .then_some("Windows treats Ctrl+Alt as AltGr, so this may block typing characters on some layouts.")
    }
}

fn key_index(name: &str) -> Option<usize> {
    KEYS.iter().position(|(key, _, _)| key.eq_ignore_ascii_case(name))
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let modifiers = [
            (self.super_key, SUPER_LABEL),
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ];

        for (enabled, name) in modifiers {
            if enabled {
                write!(f, "{name}+")?;
            }
        }

        f.write_str(KEYS[self.key].0)
    }
}

impl FromStr for Shortcut {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut shortcut = Self {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
            key: 0,
        };
        let mut key = None;

        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => shortcut.ctrl = true,
                "alt" | "option" => shortcut.alt = true,
                "shift" => shortcut.shift = true,
                "win" | "super" | "meta" | "cmd" | "command" => shortcut.super_key = true,
                _ if key.is_some() => return Err(format!("more than one key in {text:?}")),
                _ => key = Some(key_index(part).ok_or_else(|| format!("unknown key {part:?} in {text:?}"))?),
            }
        }

        shortcut.key = key.ok_or_else(|| format!("no key in {text:?}"))?;
        Ok(shortcut)
    }
}

impl Serialize for Shortcut {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Shortcut {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_shortcuts() {
        let shortcut: Shortcut = "ctrl + shift + f5".parse().expect("valid shortcut");

        assert!(shortcut.ctrl && shortcut.shift && !shortcut.alt && !shortcut.super_key);
        assert_eq!(shortcut.to_string(), "Ctrl+Shift+F5");
    }

    #[test]
    fn super_key_has_several_names() {
        for name in ["Win+O", "Super+O", "Meta+O", "Cmd+O"] {
            let shortcut: Shortcut = name.parse().expect("valid shortcut");

            assert!(shortcut.super_key, "{name}");
        }
    }

    #[test]
    fn default_is_super_shift_o() {
        let shortcut = Shortcut::default();

        assert_eq!(shortcut.to_string(), format!("{SUPER_LABEL}+Shift+O"));
        assert!(shortcut.problem().is_none());
    }

    #[test]
    fn rejects_invalid_text() {
        assert!("Ctrl+Shift".parse::<Shortcut>().is_err());
        assert!("Ctrl+A+B".parse::<Shortcut>().is_err());
        assert!("Ctrl+Hyper".parse::<Shortcut>().is_err());
    }

    #[test]
    fn requires_a_modifier_other_than_shift() {
        let bare: Shortcut = "O".parse().expect("valid shortcut");
        let shift_only: Shortcut = "Shift+O".parse().expect("valid shortcut");

        assert!(bare.problem().is_some());
        assert!(shift_only.problem().is_some());
    }

    #[test]
    fn builds_shortcuts_from_key_presses() {
        let modifiers = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };

        let shortcut = Shortcut::from_key_press(egui::Key::F9, modifiers, true).expect("supported key");

        assert_eq!(shortcut.to_string(), format!("{SUPER_LABEL}+Ctrl+Shift+F9"));
        assert!(Shortcut::from_key_press(egui::Key::Backtick, modifiers, false).is_none());
    }
}
