// lispm-remote: a browser's keys as display input.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a browser key press means to LispM.  A key event names its key
//! (`key`, what it types in the layout) and its place (`code`, the
//! physical key).  Option on a Mac types other characters (Option-x is
//! "≈"), so with Alt held the physical key decides, as Meta needs.  The
//! Command key is left to the browser (copy, paste, tabs).

use lispm_remote_display::Input;
use lispm_remote_display::Key;
use lispm_remote_display::KeyCode;
use lispm_remote_display::Mods;
use lispm_remote_display::Named;

/// A key event's fields.
#[derive(Clone, Copy, Debug)]
pub struct KeyFields<'a> {
    /// `KeyboardEvent.key`.
    pub key: &'a str,
    /// `KeyboardEvent.code`.
    pub code: &'a str,
    /// Control held.
    pub ctrl: bool,
    /// Alt (Option) held.
    pub alt: bool,
    /// Meta (Command) held.
    pub meta: bool,
    /// Shift held.
    pub shift: bool,
}

/// The input a key press is, or `None` when the page should leave it to
/// the browser or to an input method.
#[must_use]
pub fn key_input(fields: KeyFields<'_>) -> Option<Input> {
    if fields.meta {
        return None;
    }
    let mods = [
        (fields.shift, Mods::SHIFT),
        (fields.alt, Mods::ALT),
        (fields.ctrl, Mods::CTRL),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(Mods::NONE, |mods, (_, modifier)| mods.with(modifier));
    let code = named(fields.key).map(KeyCode::Named).or_else(|| {
        let typed = single(fields.key);
        let physical = (fields.alt || fields.ctrl)
            .then(|| physical(fields.code, fields.shift))
            .flatten();
        physical.or(typed).map(KeyCode::Char)
    })?;
    Some(Input::Key(Key { code, mods }))
}

/// Whether a key press with these fields types text an input method
/// should handle: a printable character with no modifier but Shift.
#[must_use]
pub fn types_text(fields: KeyFields<'_>) -> bool {
    !fields.ctrl && !fields.alt && !fields.meta && single(fields.key).is_some()
}

fn single(key: &str) -> Option<char> {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Some(c),
        _ => None,
    }
}

fn named(key: &str) -> Option<Named> {
    Some(match key {
        "Enter" => Named::Enter,
        "Tab" => Named::Tab,
        "Backspace" => Named::Backspace,
        "Escape" => Named::Escape,
        "ArrowUp" => Named::Up,
        "ArrowDown" => Named::Down,
        "ArrowLeft" => Named::Left,
        "ArrowRight" => Named::Right,
        "Home" => Named::Home,
        "End" => Named::End,
        "PageUp" => Named::PageUp,
        "PageDown" => Named::PageDown,
        "Insert" => Named::Insert,
        "Delete" => Named::Delete,
        _ => {
            let n: u8 = key.strip_prefix('F')?.parse().ok()?;
            return (1..=12).contains(&n).then_some(Named::F(n));
        }
    })
}

/// The character the physical key `code` types on a US layout.
fn physical(code: &str, shift: bool) -> Option<char> {
    if let Some(letter) = code.strip_prefix("Key").and_then(single) {
        return Some(if shift {
            letter
        } else {
            letter.to_ascii_lowercase()
        });
    }
    if let Some(digit) = code.strip_prefix("Digit").and_then(single) {
        let shifted = ")!@#$%^&*(";
        let index = usize::try_from(digit.to_digit(10)?).ok()?;
        return if shift {
            shifted.chars().nth(index)
        } else {
            Some(digit)
        };
    }
    let (plain, shifted) = match code {
        "Minus" => ('-', '_'),
        "Equal" => ('=', '+'),
        "BracketLeft" => ('[', '{'),
        "BracketRight" => (']', '}'),
        "Backslash" => ('\\', '|'),
        "Semicolon" => (';', ':'),
        "Quote" => ('\'', '"'),
        "Comma" => (',', '<'),
        "Period" => ('.', '>'),
        "Slash" => ('/', '?'),
        "Backquote" => ('`', '~'),
        "Space" => (' ', ' '),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: &str, code: &str, ctrl: bool, alt: bool, shift: bool) -> Option<Input> {
        key_input(KeyFields {
            key,
            code,
            ctrl,
            alt,
            meta: false,
            shift,
        })
    }

    fn key(c: char, mods: Mods) -> Option<Input> {
        Some(Input::Key(Key {
            code: KeyCode::Char(c),
            mods,
        }))
    }

    #[test]
    fn option_keys_mean_meta_on_the_physical_key() {
        assert_eq!(
            press("\u{2248}", "KeyX", false, true, false),
            key('x', Mods::ALT)
        );
        assert_eq!(
            press("\u{2026}", "Semicolon", true, true, false),
            key(';', Mods::ALT.with(Mods::CTRL))
        );
        assert_eq!(
            press("!", "Digit1", true, false, true),
            key('!', Mods::CTRL.with(Mods::SHIFT))
        );
    }

    #[test]
    fn named_keys_and_plain_characters_pass_through() {
        assert_eq!(
            press("ArrowLeft", "ArrowLeft", true, false, false),
            Some(Input::Key(Key {
                code: KeyCode::Named(Named::Left),
                mods: Mods::CTRL
            }))
        );
        assert_eq!(
            press("a", "KeyA", false, false, false),
            key('a', Mods::NONE)
        );
        assert_eq!(press("F13", "F13", false, false, false), None);
    }

    #[test]
    fn command_and_modifiers_alone_are_left_to_the_browser() {
        assert_eq!(
            key_input(KeyFields {
                key: "c",
                code: "KeyC",
                ctrl: false,
                alt: false,
                meta: true,
                shift: false
            }),
            None
        );
        assert_eq!(press("Shift", "ShiftLeft", false, false, true), None);
        assert_eq!(press("Process", "KeyA", false, false, false), None);
    }

    #[test]
    fn only_plain_characters_count_as_typing() {
        let fields = |key, ctrl| KeyFields {
            key,
            code: "KeyA",
            ctrl,
            alt: false,
            meta: false,
            shift: false,
        };
        assert!(types_text(fields("a", false)));
        assert!(!types_text(fields("a", true)));
        assert!(!types_text(fields("Enter", false)));
    }
}
