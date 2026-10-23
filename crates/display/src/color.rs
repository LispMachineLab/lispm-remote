// lispm-remote: colours of cells.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Colours.  A cell's colour is either the client's default (its theme's
//! foreground or background) or an exact RGB value: indexed terminal
//! colours are resolved to RGB before they reach a client, so a client
//! needs no palette.

/// A colour as three 8-bit channels.
///
/// Law: `Copy` is sound because a colour is a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// The colour `(r, g, b)`.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// The colour of a cell's text or background.
///
/// Law: `Copy` is sound because a colour is a value without identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    /// The client's default for the role (foreground or background).
    #[default]
    Default,
    /// Exactly this colour.
    Rgb(Rgb),
}

/// The RGB value xterm gives colour `index` of its 256-colour palette:
/// 16 named colours, a 6×6×6 cube, then 24 greys.
///
/// ```text
///   cube(16 + 36r + 6g + b) = (level r, level g, level b)
///   level 0 = 0, level k = 55 + 40k        (k ∈ 1..=5)
///   grey(232 + i)           = 8 + 10i      (i ∈ 0..24)
/// ```
#[must_use]
pub const fn xterm_color(index: u8) -> Rgb {
    match index {
        0..=15 => named(index),
        16..=231 => {
            let n = index - 16;
            Rgb::new(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let grey = 8 + 10 * (index - 232);
            Rgb::new(grey, grey, grey)
        }
    }
}

/// Pre: `k ≤ 5`.  Post: the cube's channel value for step `k`.
const fn level(k: u8) -> u8 {
    if k == 0 { 0 } else { 55 + 40 * k }
}

/// The 16 named colours, as xterm's defaults set them.
const fn named(index: u8) -> Rgb {
    match index {
        0 => Rgb::new(0, 0, 0),
        1 => Rgb::new(205, 0, 0),
        2 => Rgb::new(0, 205, 0),
        3 => Rgb::new(205, 205, 0),
        4 => Rgb::new(0, 0, 238),
        5 => Rgb::new(205, 0, 205),
        6 => Rgb::new(0, 205, 205),
        7 => Rgb::new(229, 229, 229),
        8 => Rgb::new(127, 127, 127),
        9 => Rgb::new(255, 0, 0),
        10 => Rgb::new(0, 255, 0),
        11 => Rgb::new(255, 255, 0),
        12 => Rgb::new(92, 92, 255),
        13 => Rgb::new(255, 0, 255),
        14 => Rgb::new(0, 255, 255),
        _ => Rgb::new(255, 255, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_has_xterms_landmarks() {
        let table = [
            (0, Rgb::new(0, 0, 0)),
            (9, Rgb::new(255, 0, 0)),
            (15, Rgb::new(255, 255, 255)),
            (16, Rgb::new(0, 0, 0)),
            (21, Rgb::new(0, 0, 255)),
            (196, Rgb::new(255, 0, 0)),
            (231, Rgb::new(255, 255, 255)),
            (232, Rgb::new(8, 8, 8)),
            (255, Rgb::new(238, 238, 238)),
        ];
        for (index, rgb) in table {
            assert_eq!(xterm_color(index), rgb, "colour {index}");
        }
    }

    #[test]
    fn the_cube_is_monotone_in_each_channel() {
        for k in 0..5 {
            assert!(level(k) < level(k + 1));
        }
    }
}
