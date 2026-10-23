// lispm-remote: how a cell's text looks.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Styles: colours, attributes and the kind of underline.

use crate::color::Color;

/// A set of text attributes, one bit each.
///
/// Law: `Copy` is sound because a set of flags is a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attrs(u16);

impl Attrs {
    /// No attribute.
    pub const NONE: Self = Self(0);
    /// Bold, or bright.
    pub const BOLD: Self = Self(1);
    /// Faint.
    pub const DIM: Self = Self(1 << 1);
    /// Italic.
    pub const ITALIC: Self = Self(1 << 2);
    /// Foreground and background swapped.
    pub const INVERSE: Self = Self(1 << 3);
    /// Drawn in the background colour.
    pub const INVISIBLE: Self = Self(1 << 4);
    /// Struck through.
    pub const STRIKE: Self = Self(1 << 5);
    /// Blinking.
    pub const BLINK: Self = Self(1 << 6);
    /// Overlined.
    pub const OVERLINE: Self = Self(1 << 7);
    /// Every defined attribute.
    pub const ALL: Self = Self((1 << 8) - 1);

    /// The set whose bits are `bits`, keeping only defined attributes.
    #[must_use]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// The set's bits.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether every attribute of `other` is in this set.
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
}

/// How text is underlined.
///
/// Law: `Copy` is sound because the kind is a tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    /// Not underlined.
    #[default]
    None,
    /// One straight line.
    Single,
    /// Two straight lines.
    Double,
    /// A wave.
    Curly,
    /// Dots.
    Dotted,
    /// Dashes.
    Dashed,
}

/// How a cell's text looks.
///
/// Law: `Copy` is sound because a style is a value without identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    /// The text's colour.
    pub fg: Color,
    /// The cell's background.
    pub bg: Color,
    /// The attributes.
    pub attrs: Attrs,
    /// The underline.
    pub underline: Underline,
    /// The underline's colour; `Default` means the text's.
    pub underline_color: Color,
}

impl Style {
    /// The style with nothing set: default colours, no attribute.
    pub const PLAIN: Self = Self {
        fg: Color::Default,
        bg: Color::Default,
        attrs: Attrs::NONE,
        underline: Underline::None,
        underline_color: Color::Default,
    };

    /// The style an erased cell takes under this pen: its background
    /// only (background colour erase).
    #[must_use]
    pub const fn erased(self) -> Self {
        Self {
            bg: self.bg,
            ..Self::PLAIN
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    #[test]
    fn with_and_without_are_inverse_on_a_set_without_the_flag() {
        let set = Attrs::ITALIC;
        assert_eq!(set.with(Attrs::BOLD).without(Attrs::BOLD), set);
        assert!(set.with(Attrs::BOLD).contains(Attrs::BOLD));
    }

    #[test]
    fn from_bits_drops_undefined_bits() {
        assert_eq!(Attrs::from_bits(u16::MAX), Attrs::ALL);
    }

    #[test]
    fn an_erased_cell_keeps_only_the_background() {
        let pen = Style {
            fg: Color::Rgb(Rgb::new(1, 2, 3)),
            bg: Color::Rgb(Rgb::new(4, 5, 6)),
            attrs: Attrs::BOLD,
            underline: Underline::Curly,
            underline_color: Color::Rgb(Rgb::new(7, 8, 9)),
        };
        assert_eq!(
            pen.erased(),
            Style {
                bg: pen.bg,
                ..Style::PLAIN
            }
        );
    }
}
