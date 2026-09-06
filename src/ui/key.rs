//! Frontend-neutral key presses for the overlay UI.
//!
//! The SDL binary maps `sdl2::keyboard::Keycode` to [`Key`] (printable
//! ASCII codes 32..=126 become [`Key::Char`], the named keys map one to
//! one, everything else is [`Key::Other`]); the web page maps
//! `KeyboardEvent.code` through [`Key::from_browser_code`]. Both ignore
//! Shift: the palette matches case-insensitively and the two shifted
//! characters cheat codes need (`:` and `?`) are typed as `;` and `/`
//! (docs/debugging/UI_FRAMEWORK.md).
//!
//! Space is `Char(' ')`. The backquote is never printable (it toggles the
//! palette). The keypad Enter folds into [`Key::Return`].

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable ASCII character, lower case for letters.
    Char(char),
    Backquote,
    Escape,
    Return,
    Backspace,
    Delete,
    Insert,
    Tab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    /// Any key the UI has no use for (modifiers, F9 and up, media keys).
    Other,
}

impl Key {
    /// The character this key types into a text entry, if any.
    pub fn printable(self) -> Option<char> {
        match self {
            Key::Char(c) => Some(c),
            _ => None,
        }
    }

    /// The digit 0-9 this key types, if any.
    pub fn digit(self) -> Option<u8> {
        self.printable()
            .and_then(|c| c.to_digit(10))
            .map(|d| d as u8)
    }

    // Unused by the SDL binary; the web page calls it once this module
    // lives in the library (docs/plans/SHARED_OVERLAY_UI.md, Phase 3).
    #[allow(dead_code)]
    /// Map a browser `KeyboardEvent.code` (a physical key name such as
    /// `KeyA`, `Digit3`, `ArrowUp`, `F5`) to a [`Key`]. Unknown codes are
    /// [`Key::Other`].
    pub fn from_browser_code(code: &str) -> Key {
        if let Some(letter) = code.strip_prefix("Key") {
            let mut chars = letter.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_uppercase() {
                    return Key::Char(c.to_ascii_lowercase());
                }
            }
            return Key::Other;
        }
        if let Some(digit) = code
            .strip_prefix("Digit")
            .or_else(|| code.strip_prefix("Numpad"))
        {
            let mut chars = digit.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_digit() {
                    return Key::Char(c);
                }
            }
        }
        match code {
            "Space" => Key::Char(' '),
            "Minus" | "NumpadSubtract" => Key::Char('-'),
            "Equal" => Key::Char('='),
            "NumpadAdd" => Key::Char('+'),
            "NumpadMultiply" => Key::Char('*'),
            "NumpadDivide" | "Slash" => Key::Char('/'),
            "NumpadDecimal" | "Period" => Key::Char('.'),
            "BracketLeft" => Key::Char('['),
            "BracketRight" => Key::Char(']'),
            "Backslash" => Key::Char('\\'),
            "Semicolon" => Key::Char(';'),
            "Quote" => Key::Char('\''),
            "Comma" => Key::Char(','),
            "Backquote" => Key::Backquote,
            "Escape" => Key::Escape,
            "Enter" | "NumpadEnter" => Key::Return,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Insert" => Key::Insert,
            "Tab" => Key::Tab,
            "ArrowUp" => Key::Up,
            "ArrowDown" => Key::Down,
            "ArrowLeft" => Key::Left,
            "ArrowRight" => Key::Right,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            "Home" => Key::Home,
            "End" => Key::End,
            "F1" => Key::F1,
            "F2" => Key::F2,
            "F3" => Key::F3,
            "F4" => Key::F4,
            "F5" => Key::F5,
            "F6" => Key::F6,
            "F7" => Key::F7,
            "F8" => Key::F8,
            _ => Key::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_codes_map_letters_digits_and_punctuation() {
        assert_eq!(Key::from_browser_code("KeyA"), Key::Char('a'));
        assert_eq!(Key::from_browser_code("KeyZ"), Key::Char('z'));
        assert_eq!(Key::from_browser_code("Digit0"), Key::Char('0'));
        assert_eq!(Key::from_browser_code("Digit7"), Key::Char('7'));
        assert_eq!(Key::from_browser_code("Numpad5"), Key::Char('5'));
        assert_eq!(Key::from_browser_code("Space"), Key::Char(' '));
        assert_eq!(Key::from_browser_code("Minus"), Key::Char('-'));
        assert_eq!(Key::from_browser_code("Equal"), Key::Char('='));
        assert_eq!(Key::from_browser_code("NumpadAdd"), Key::Char('+'));
        assert_eq!(Key::from_browser_code("Semicolon"), Key::Char(';'));
        assert_eq!(Key::from_browser_code("Slash"), Key::Char('/'));
        assert_eq!(Key::from_browser_code("Quote"), Key::Char('\''));
        assert_eq!(Key::from_browser_code("Comma"), Key::Char(','));
        assert_eq!(Key::from_browser_code("Period"), Key::Char('.'));
    }

    #[test]
    fn browser_codes_map_named_keys() {
        assert_eq!(Key::from_browser_code("Backquote"), Key::Backquote);
        assert_eq!(Key::from_browser_code("Escape"), Key::Escape);
        assert_eq!(Key::from_browser_code("Enter"), Key::Return);
        assert_eq!(Key::from_browser_code("NumpadEnter"), Key::Return);
        assert_eq!(Key::from_browser_code("Backspace"), Key::Backspace);
        assert_eq!(Key::from_browser_code("Delete"), Key::Delete);
        assert_eq!(Key::from_browser_code("Insert"), Key::Insert);
        assert_eq!(Key::from_browser_code("Tab"), Key::Tab);
        assert_eq!(Key::from_browser_code("ArrowUp"), Key::Up);
        assert_eq!(Key::from_browser_code("ArrowDown"), Key::Down);
        assert_eq!(Key::from_browser_code("ArrowLeft"), Key::Left);
        assert_eq!(Key::from_browser_code("ArrowRight"), Key::Right);
        assert_eq!(Key::from_browser_code("PageUp"), Key::PageUp);
        assert_eq!(Key::from_browser_code("PageDown"), Key::PageDown);
        assert_eq!(Key::from_browser_code("Home"), Key::Home);
        assert_eq!(Key::from_browser_code("End"), Key::End);
        assert_eq!(Key::from_browser_code("F1"), Key::F1);
        assert_eq!(Key::from_browser_code("F5"), Key::F5);
        assert_eq!(Key::from_browser_code("F8"), Key::F8);
    }

    #[test]
    fn unknown_codes_are_other() {
        for code in [
            "F9",
            "F12",
            "ShiftLeft",
            "ShiftRight",
            "ControlLeft",
            "MetaLeft",
            "AltRight",
            "CapsLock",
            "KeyAB",
            "Key1",
            "Digit",
            "DigitX",
            "",
            "Unidentified",
        ] {
            assert_eq!(Key::from_browser_code(code), Key::Other, "{code}");
        }
    }

    #[test]
    fn printable_and_digit() {
        assert_eq!(Key::Char('a').printable(), Some('a'));
        assert_eq!(Key::Char(' ').printable(), Some(' '));
        assert_eq!(Key::Backquote.printable(), None);
        assert_eq!(Key::F1.printable(), None);
        assert_eq!(Key::Return.printable(), None);
        assert_eq!(Key::Char('3').digit(), Some(3));
        assert_eq!(Key::Char('a').digit(), None);
        assert_eq!(Key::Up.digit(), None);
    }
}
