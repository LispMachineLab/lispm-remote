// lispm-remote: what a person does on a client.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Input, as the client saw it: keys with their modifiers, text an input
//! method committed, the pointer over cells, and focus.  Turning it into
//! what Emacs reads is the relay's business.

/// Modifier keys held, one bit each.
///
/// Law: `Copy` is sound because a set of flags is a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(u8);

impl Mods {
    /// None held.
    pub const NONE: Self = Self(0);
    /// Shift.
    pub const SHIFT: Self = Self(1);
    /// Alt (Option), which Emacs reads as Meta.
    pub const ALT: Self = Self(1 << 1);
    /// Control.
    pub const CTRL: Self = Self(1 << 2);
    /// Super (Command).
    pub const SUPER: Self = Self(1 << 3);
    /// Every defined modifier.
    pub const ALL: Self = Self((1 << 4) - 1);

    /// The set whose bits are `bits`, keeping only defined modifiers.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// The set's bits.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every modifier of `other` is held.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// This set with `other` added.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// This set with `other` taken away.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Whether no modifier is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A key without a character of its own.
///
/// Law: `Copy` is sound because a key name is a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Named {
    /// Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace (Delete on a Mac keyboard).
    Backspace,
    /// Escape.
    Escape,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Insert.
    Insert,
    /// Forward delete.
    Delete,
    /// A function key, F1 to F12.
    F(u8),
}

/// What key was pressed.
///
/// Law: `Copy` is sound because a key is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A key that types `char` (unshifted for letters when Ctrl is held).
    Char(char),
    /// A key without a character.
    Named(Named),
}

/// A key pressed with modifiers.
///
/// Law: `Copy` is sound because a key press is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// The key.
    pub code: KeyCode,
    /// The modifiers held.
    pub mods: Mods,
}

/// A pointer button, or the wheel.
///
/// Law: `Copy` is sound because a button is a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    /// The primary button (a tap).
    Left,
    /// The middle button.
    Middle,
    /// The secondary button.
    Right,
    /// The wheel turned up.
    WheelUp,
    /// The wheel turned down.
    WheelDown,
    /// The wheel turned left.
    WheelLeft,
    /// The wheel turned right.
    WheelRight,
    /// No button: the pointer moved.
    None,
}

/// What the pointer did.
///
/// Law: `Copy` is sound because the kind is a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerKind {
    /// A button went down (or the wheel turned).
    Press,
    /// A button came up.
    Release,
    /// The pointer moved, with `button` held or none.
    Motion,
}

/// The pointer over a cell.
///
/// Law: `Copy` is sound because a pointer event is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pointer {
    /// What happened.
    pub kind: PointerKind,
    /// Which button.
    pub button: Button,
    /// The cell's column.
    pub col: u16,
    /// The cell's row.
    pub row: u16,
    /// The modifiers held.
    pub mods: Mods,
}

/// One thing a person did on a surface.
///
/// Law: `Clone` duplicates a value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    /// A key.
    Key(Key),
    /// Text an input method committed, or that was pasted.
    Text(String),
    /// The pointer.
    Pointer(Pointer),
    /// The surface gained (`true`) or lost focus.
    Focus(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_combine_and_part() {
        let held = Mods::CTRL.with(Mods::ALT);
        assert!(held.contains(Mods::CTRL) && held.contains(Mods::ALT));
        assert!(!held.contains(Mods::SHIFT));
        assert_eq!(held.without(Mods::ALT), Mods::CTRL);
        assert!(Mods::NONE.is_empty());
        assert_eq!(Mods::from_bits(u8::MAX), Mods::ALL);
    }
}
