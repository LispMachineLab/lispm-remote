// lispm-remote: Select Graphic Rendition.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SGR: how the pen changes.  Colours come in every form Emacs writes:
//! `38;5;n` and `38;2;r;g;b` (separate parameters), `38:5:n` and
//! `38:2::r:g:b` (sub-parameters); underline kinds as `4:n` (Kitty's
//! form, which Emacs uses for waves).  Indexed colours become RGB here.

use lispm_remote_display::Attrs;
use lispm_remote_display::Color;
use lispm_remote_display::Rgb;
use lispm_remote_display::Style;
use lispm_remote_display::Underline;
use lispm_remote_display::xterm_color;

/// The pen after the SGR whose parameters are `params`.
pub(super) fn apply(pen: Style, params: &vte::Params) -> Style {
    if params.is_empty() {
        return Style::PLAIN;
    }
    let mut pen = pen;
    let mut params = params.iter();
    while let Some(param) = params.next() {
        match param {
            [] | [0] => pen = Style::PLAIN,
            [4, kind, ..] => pen.underline = underline(*kind),
            [38, rest @ ..] => pen.fg = color(rest, &mut params).unwrap_or(pen.fg),
            [48, rest @ ..] => pen.bg = color(rest, &mut params).unwrap_or(pen.bg),
            [58, rest @ ..] => {
                pen.underline_color = color(rest, &mut params).unwrap_or(pen.underline_color)
            }
            [code, ..] => pen = simple(pen, *code),
        }
    }
    pen
}

/// The pen after the one-number SGR `code`.
fn simple(pen: Style, code: u16) -> Style {
    let attrs = pen.attrs;
    let with = |flag| Style {
        attrs: attrs.with(flag),
        ..pen
    };
    let without = |flag| Style {
        attrs: attrs.without(flag),
        ..pen
    };
    match code {
        1 => with(Attrs::BOLD),
        2 => with(Attrs::DIM),
        3 => with(Attrs::ITALIC),
        4 => Style {
            underline: Underline::Single,
            ..pen
        },
        5 | 6 => with(Attrs::BLINK),
        7 => with(Attrs::INVERSE),
        8 => with(Attrs::INVISIBLE),
        9 => with(Attrs::STRIKE),
        21 => Style {
            underline: Underline::Double,
            ..pen
        },
        22 => without(Attrs::BOLD.with(Attrs::DIM)),
        23 => without(Attrs::ITALIC),
        24 => Style {
            underline: Underline::None,
            ..pen
        },
        25 => without(Attrs::BLINK),
        27 => without(Attrs::INVERSE),
        28 => without(Attrs::INVISIBLE),
        29 => without(Attrs::STRIKE),
        30..=37 => Style {
            fg: indexed(code - 30),
            ..pen
        },
        39 => Style {
            fg: Color::Default,
            ..pen
        },
        40..=47 => Style {
            bg: indexed(code - 40),
            ..pen
        },
        49 => Style {
            bg: Color::Default,
            ..pen
        },
        53 => with(Attrs::OVERLINE),
        55 => without(Attrs::OVERLINE),
        59 => Style {
            underline_color: Color::Default,
            ..pen
        },
        90..=97 => Style {
            fg: indexed(code - 82),
            ..pen
        },
        100..=107 => Style {
            bg: indexed(code - 92),
            ..pen
        },
        _ => pen,
    }
}

const fn underline(kind: u16) -> Underline {
    match kind {
        0 => Underline::None,
        2 => Underline::Double,
        3 => Underline::Curly,
        4 => Underline::Dotted,
        5 => Underline::Dashed,
        _ => Underline::Single,
    }
}

/// Palette colour `index` as RGB.
fn indexed(index: u16) -> Color {
    u8::try_from(index).map_or(Color::Default, |index| Color::Rgb(xterm_color(index)))
}

/// An extended colour: from the sub-parameters `rest` when there are
/// any, else from the parameters that follow (consumed).
fn color<'a>(rest: &[u16], params: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    match rest {
        [5, index, ..] => Some(indexed(*index)),
        // 38:2:r:g:b or 38:2:colourspace:r:g:b.
        [2, _, r, g, b, ..] | [2, r, g, b] => Some(rgb(*r, *g, *b)),
        [] => {
            let first = params.next()?.first().copied()?;
            match first {
                5 => Some(indexed(params.next()?.first().copied()?)),
                2 => {
                    let mut channel = || params.next().and_then(|p| p.first().copied());
                    let (r, g, b) = (channel()?, channel()?, channel()?);
                    Some(rgb(r, g, b))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn rgb(r: u16, g: u16, b: u16) -> Color {
    let channel = |value: u16| u8::try_from(value).unwrap_or(u8::MAX);
    Color::Rgb(Rgb::new(channel(r), channel(g), channel(b)))
}
