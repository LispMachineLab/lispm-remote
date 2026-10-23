// lispm-remote: input, as Emacs reads it from a terminal.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a client's input becomes on the terminal's input side: the bytes
//! an xterm would send, in the modes Emacs set.
//!
//! ```text
//!   Char c                 c in UTF-8; Ctrl+letter → control code
//!   Ctrl/Meta + symbol     ESC [ 27 ; m ; code ~   when xterm.el binds it
//!   Meta                   ESC prefix
//!   arrows, Home, End      ESC [ X  or ESC O X (DECCKM);  ESC [ 1 ; m X with modifiers
//!   Text t                 ESC [ 200 ~ t ESC [ 201 ~ in bracketed paste, else t
//!   Pointer                ESC [ < b ; col ; row M/m (SGR), when Emacs asked for it
//!   Focus                  ESC [ I  /  ESC [ O, when Emacs asked for it
//! ```
//!
//! `m` is xterm's modifier number: 1 + Shift + 2·Meta + 4·Ctrl.  Super has
//! no form on a terminal, so a key with Super sends nothing.

use lispm_remote_display::Button;
use lispm_remote_display::Input;
use lispm_remote_display::Key;
use lispm_remote_display::KeyCode;
use lispm_remote_display::Mods;
use lispm_remote_display::Named;
use lispm_remote_display::Pointer;
use lispm_remote_display::PointerKind;

use crate::terminal::Modes;
use crate::terminal::MouseMode;

/// The `(modifier, character)` pairs Emacs's xterm.el binds in the
/// `ESC [ 27 ; m ; c ~` form, from its modifyOtherKeys table.
const MODIFY_OTHER_KEYS: &[(u8, u32)] = &[
    (5, 9),
    (5, 13),
    (5, 39),
    (5, 44),
    (5, 45),
    (5, 46),
    (5, 47),
    (5, 48),
    (5, 49),
    (5, 57),
    (5, 59),
    (5, 61),
    (5, 92),
    (6, 33),
    (6, 34),
    (6, 35),
    (6, 36),
    (6, 37),
    (6, 38),
    (6, 40),
    (6, 41),
    (6, 42),
    (6, 43),
    (6, 58),
    (6, 60),
    (6, 62),
    (6, 63),
    (7, 9),
    (7, 13),
    (7, 32),
    (7, 39),
    (7, 44),
    (7, 45),
    (7, 46),
    (7, 47),
    (7, 48),
    (7, 49),
    (7, 50),
    (7, 51),
    (7, 52),
    (7, 53),
    (7, 54),
    (7, 55),
    (7, 56),
    (7, 57),
    (7, 59),
    (7, 61),
    (7, 92),
    (8, 33),
    (8, 34),
    (8, 35),
    (8, 36),
    (8, 37),
    (8, 38),
    (8, 40),
    (8, 41),
    (8, 42),
    (8, 43),
    (8, 58),
    (8, 60),
    (8, 62),
    (8, 63),
    (3, 32),
    (2, 9),
    (2, 13),
    (6, 9),
    (6, 13),
];

const ESC: u8 = 0x1b;

/// xterm's modifier number for `mods`, Super left out.
fn modifier(mods: Mods) -> u8 {
    1 + u8::from(mods.contains(Mods::SHIFT))
        + 2 * u8::from(mods.contains(Mods::ALT))
        + 4 * u8::from(mods.contains(Mods::CTRL))
}

/// Whether Emacs binds `ESC [ 27 ; m ; code ~`.
fn emacs_binds(m: u8, code: u32) -> bool {
    MODIFY_OTHER_KEYS.contains(&(m, code))
}

/// The bytes `input` sends to Emacs in `modes`.
#[must_use]
pub fn encode(input: &Input, modes: &Modes) -> Vec<u8> {
    match input {
        Input::Key(key) => encode_key(*key, modes),
        Input::Text(text) => encode_text(text, modes),
        Input::Pointer(pointer) => encode_pointer(*pointer, modes),
        Input::Focus(focused) => {
            if modes.focus {
                if *focused {
                    b"\x1b[I".to_vec()
                } else {
                    b"\x1b[O".to_vec()
                }
            } else {
                Vec::new()
            }
        }
    }
}

fn encode_key(key: Key, modes: &Modes) -> Vec<u8> {
    if key.mods.contains(Mods::SUPER) {
        return Vec::new();
    }
    match key.code {
        KeyCode::Char(c) => encode_char(c, key.mods),
        KeyCode::Named(named) => encode_named(named, key.mods, modes),
    }
}

fn encode_char(c: char, mods: Mods) -> Vec<u8> {
    let m = modifier(mods);
    let code = u32::from(c);
    if m > 1 && emacs_binds(m, code) {
        return format!("\x1b[27;{m};{code}~").into_bytes();
    }
    let mut bytes = Vec::new();
    if mods.contains(Mods::ALT) {
        bytes.push(ESC);
    }
    match control(c).filter(|_| mods.contains(Mods::CTRL)) {
        Some(byte) => bytes.push(byte),
        None => {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        }
    }
    bytes
}

/// The control code Ctrl turns `c` into, if any.
fn control(c: char) -> Option<u8> {
    let byte = u8::try_from(u32::from(c)).ok()?;
    match byte {
        b'a'..=b'z' => Some(byte - b'a' + 1),
        b'A'..=b'Z' => Some(byte - b'A' + 1),
        b'@' | b' ' | b'2' => Some(0),
        b'[' | b'3' => Some(0x1b),
        b'\\' | b'4' => Some(0x1c),
        b']' | b'5' => Some(0x1d),
        b'^' | b'6' => Some(0x1e),
        b'_' | b'7' | b'/' => Some(0x1f),
        b'?' | b'8' => Some(0x7f),
        _ => None,
    }
}

fn encode_named(named: Named, mods: Mods, modes: &Modes) -> Vec<u8> {
    let m = modifier(mods);
    let meta = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if mods.contains(Mods::ALT) {
            out.push(ESC);
        }
        out.extend_from_slice(bytes);
        out
    };
    match named {
        Named::Tab if mods == Mods::SHIFT => b"\x1b[Z".to_vec(),
        Named::Enter if m > 1 && emacs_binds(m, 13) => format!("\x1b[27;{m};13~").into_bytes(),
        Named::Tab if m > 1 && emacs_binds(m, 9) => format!("\x1b[27;{m};9~").into_bytes(),
        Named::Enter => meta(b"\r"),
        Named::Tab => meta(b"\t"),
        Named::Backspace => meta(if mods.contains(Mods::CTRL) {
            b"\x08"
        } else {
            b"\x7f"
        }),
        Named::Escape => meta(b"\x1b"),
        Named::Up => cursor_key(b'A', m, modes),
        Named::Down => cursor_key(b'B', m, modes),
        Named::Right => cursor_key(b'C', m, modes),
        Named::Left => cursor_key(b'D', m, modes),
        Named::Home => cursor_key(b'H', m, modes),
        Named::End => cursor_key(b'F', m, modes),
        Named::Insert => tilde(2, m),
        Named::Delete => tilde(3, m),
        Named::PageUp => tilde(5, m),
        Named::PageDown => tilde(6, m),
        Named::F(n) => function_key(n, m),
    }
}

/// An arrow, Home or End: `ESC [ X` or `ESC O X`, or `ESC [ 1 ; m X`.
fn cursor_key(letter: u8, m: u8, modes: &Modes) -> Vec<u8> {
    if m > 1 {
        let mut bytes = format!("\x1b[1;{m}").into_bytes();
        bytes.push(letter);
        bytes
    } else {
        vec![ESC, if modes.app_cursor { b'O' } else { b'[' }, letter]
    }
}

/// `ESC [ n ~`, or `ESC [ n ; m ~` with modifiers.
fn tilde(n: u8, m: u8) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[{n};{m}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn function_key(n: u8, m: u8) -> Vec<u8> {
    match n {
        1..=4 => {
            let letter = b'P' + (n - 1);
            if m > 1 {
                let mut bytes = format!("\x1b[1;{m}").into_bytes();
                bytes.push(letter);
                bytes
            } else {
                vec![ESC, b'O', letter]
            }
        }
        5 => tilde(15, m),
        6..=10 => tilde(n + 11, m),
        11 | 12 => tilde(n + 12, m),
        _ => Vec::new(),
    }
}

fn encode_text(text: &str, modes: &Modes) -> Vec<u8> {
    // Escapes inside pasted text could end the paste early.
    let clean: String = text.chars().filter(|&c| c != '\u{1b}').collect();
    if modes.bracketed_paste {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(clean.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        clean.into_bytes()
    }
}

/// An SGR mouse report, when the mode asks for this event.
fn encode_pointer(pointer: Pointer, modes: &Modes) -> Vec<u8> {
    let wanted = match (modes.mouse, pointer.kind, pointer.button) {
        (MouseMode::Off, _, _) => false,
        (_, PointerKind::Press | PointerKind::Release, _) => true,
        (MouseMode::Drag, PointerKind::Motion, button) => button != Button::None,
        (MouseMode::Any, PointerKind::Motion, _) => true,
        (MouseMode::Press, PointerKind::Motion, _) => false,
    };
    if !wanted || !modes.sgr_mouse {
        return Vec::new();
    }
    let base: u16 = match pointer.button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
        Button::WheelLeft => 66,
        Button::WheelRight => 67,
    };
    let mods = pointer.mods;
    let code = base
        + 4 * u16::from(mods.contains(Mods::SHIFT))
        + 8 * u16::from(mods.contains(Mods::ALT))
        + 16 * u16::from(mods.contains(Mods::CTRL))
        + if pointer.kind == PointerKind::Motion {
            32
        } else {
            0
        };
    let end = if pointer.kind == PointerKind::Release {
        'm'
    } else {
        'M'
    };
    format!(
        "\x1b[<{code};{};{}{end}",
        u32::from(pointer.col) + 1,
        u32::from(pointer.row) + 1
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: Mods) -> Input {
        Input::Key(Key { code, mods })
    }

    fn plain() -> Modes {
        Modes::default()
    }

    #[test]
    fn keys_send_what_an_xterm_sends() {
        let table: [(Input, &[u8]); 14] = [
            (key(KeyCode::Char('a'), Mods::NONE), b"a"),
            (key(KeyCode::Char('x'), Mods::CTRL), b"\x18"),
            (key(KeyCode::Char('x'), Mods::ALT), b"\x1bx"),
            (
                key(KeyCode::Char('x'), Mods::CTRL.with(Mods::ALT)),
                b"\x1b\x18",
            ),
            (key(KeyCode::Char(';'), Mods::CTRL), b"\x1b[27;5;59~"),
            (
                key(KeyCode::Char(';'), Mods::CTRL.with(Mods::ALT)),
                b"\x1b[27;7;59~",
            ),
            (
                key(KeyCode::Char('!'), Mods::CTRL.with(Mods::SHIFT)),
                b"\x1b[27;6;33~",
            ),
            (key(KeyCode::Char(' '), Mods::CTRL), b"\x00"),
            (key(KeyCode::Named(Named::Up), Mods::NONE), b"\x1b[A"),
            (key(KeyCode::Named(Named::Left), Mods::CTRL), b"\x1b[1;5D"),
            (
                key(KeyCode::Named(Named::Enter), Mods::CTRL),
                b"\x1b[27;5;13~",
            ),
            (key(KeyCode::Named(Named::Tab), Mods::SHIFT), b"\x1b[Z"),
            (key(KeyCode::Named(Named::F(5)), Mods::NONE), b"\x1b[15~"),
            (
                key(KeyCode::Named(Named::PageDown), Mods::SHIFT),
                b"\x1b[6;2~",
            ),
        ];
        for (input, bytes) in table {
            assert_eq!(encode(&input, &plain()), bytes.to_vec(), "{input:?}");
        }
    }

    #[test]
    fn a_key_with_super_sends_nothing() {
        assert!(encode(&key(KeyCode::Char('c'), Mods::SUPER), &plain()).is_empty());
    }

    #[test]
    fn cursor_keys_follow_decckm() {
        let modes = Modes {
            app_cursor: true,
            ..plain()
        };
        assert_eq!(
            encode(&key(KeyCode::Named(Named::Up), Mods::NONE), &modes),
            b"\x1bOA".to_vec()
        );
    }

    #[test]
    fn text_is_bracketed_when_emacs_asks() {
        let text = Input::Text("\u{4f60}\u{597d}\u{1b}".to_owned());
        assert_eq!(
            encode(&text, &plain()),
            "\u{4f60}\u{597d}".as_bytes().to_vec()
        );
        let modes = Modes {
            bracketed_paste: true,
            ..plain()
        };
        assert_eq!(
            encode(&text, &modes),
            "\x1b[200~\u{4f60}\u{597d}\x1b[201~".as_bytes().to_vec()
        );
    }

    #[test]
    fn the_pointer_is_reported_only_as_asked() {
        let press = Input::Pointer(Pointer {
            kind: PointerKind::Press,
            button: Button::Left,
            col: 4,
            row: 2,
            mods: Mods::NONE,
        });
        assert!(encode(&press, &plain()).is_empty());
        let modes = Modes {
            mouse: MouseMode::Drag,
            sgr_mouse: true,
            ..plain()
        };
        assert_eq!(encode(&press, &modes), b"\x1b[<0;5;3M".to_vec());
        let hover = Input::Pointer(Pointer {
            kind: PointerKind::Motion,
            button: Button::None,
            col: 0,
            row: 0,
            mods: Mods::NONE,
        });
        assert!(encode(&hover, &modes).is_empty());
        let wheel = Input::Pointer(Pointer {
            kind: PointerKind::Press,
            button: Button::WheelDown,
            col: 0,
            row: 0,
            mods: Mods::CTRL,
        });
        assert_eq!(encode(&wheel, &modes), b"\x1b[<81;1;1M".to_vec());
    }
}
