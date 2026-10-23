// lispm-remote: a terminal for Emacs to draw on.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The terminal an Emacs tty frame draws on.  Emacs believes it talks to
//! an xterm; this reads what Emacs writes and keeps the screen it
//! describes, as a [`Screen`] a client can be sent.
//!
//! ```text
//!   feed : Terminal × bytes → Terminal        (screen, modes, replies, events)
//! ```
//!
//! `feed` is pure: bytes in, state out.  Replies (answers to Emacs's
//! questions about the terminal) and events (bell, clipboard, images)
//! wait in the state until [`Terminal::take_replies`] and
//! [`Terminal::take_events`] collect them; the shell writes replies back
//! to Emacs.
//!
//! What Emacs asks and is told:
//!
//! ```text
//!   CSI c         primary attributes   VT220, colour, clipboard (52)
//!   CSI > c       secondary            xterm 390, so xterm.el turns on
//!                                      modifyOtherKeys and OSC 52
//!   CSI > q       XTVERSION            "LispM Remote"
//!   OSC 10/11 ?   colours              the client's default text and background
//!   CSI 14/16/18 t  sizes              text area, cell and grid
//!   APC G a=q     Kitty graphics       OK
//! ```

mod control;
mod graphics;
mod grid;
mod sgr;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use lispm_remote_display::CellPixels;
use lispm_remote_display::ResourceId;
use lispm_remote_display::Rgb;
use lispm_remote_display::Screen;
use lispm_remote_display::Size;
use lispm_remote_display::Style;

use crate::apc::Segment;
use crate::apc::Splitter;

/// How the terminal reports the pointer.
///
/// Law: `Copy` is sound because the mode is a tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    /// Not at all.
    #[default]
    Off,
    /// Presses and releases (mode 1000).
    Press,
    /// Those, and motion with a button held (1002).
    Drag,
    /// Those, and all motion (1003).
    Any,
}

/// The terminal modes that change what input means.
///
/// Law: `Copy` is sound because modes are flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    /// Cursor keys send `ESC O` (DECCKM).
    pub app_cursor: bool,
    /// The keypad sends application codes (DECKPAM).
    pub app_keypad: bool,
    /// Text wraps at the right margin (DECAWM).
    pub autowrap: bool,
    /// Cursor positions count from the scrolling region (DECOM).
    pub origin: bool,
    /// Printing inserts instead of overwriting (IRM).
    pub insert: bool,
    /// Pastes are bracketed (2004).
    pub bracketed_paste: bool,
    /// Focus changes are reported (1004).
    pub focus: bool,
    /// How the pointer is reported.
    pub mouse: MouseMode,
    /// Pointer reports use the SGR form (1006).
    pub sgr_mouse: bool,
    /// xterm's modifyOtherKeys level.
    pub modify_other_keys: u8,
    /// Output is held until the program finishes a change (2026).
    pub synchronized: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            app_cursor: false,
            app_keypad: false,
            autowrap: true,
            origin: false,
            insert: false,
            bracketed_paste: false,
            focus: false,
            mouse: MouseMode::Off,
            sgr_mouse: false,
            modify_other_keys: 0,
            synchronized: false,
        }
    }
}

/// What the terminal tells Emacs about the client.
///
/// Law: `Copy` is sound because options are values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// A cell's size in pixels.
    pub cell: CellPixels,
    /// The client's default text colour.
    pub foreground: Rgb,
    /// The client's default background.
    pub background: Rgb,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cell: CellPixels {
                width: 8,
                height: 16,
            },
            foreground: Rgb::new(0, 0, 0),
            background: Rgb::new(255, 255, 255),
        }
    }
}

/// Something that happened besides the screen changing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The bell rang.
    Bell,
    /// Emacs put text on the clipboard.
    Clipboard(String),
    /// An image arrived: its id and bytes.
    Resource(ResourceId, Arc<[u8]>),
}

/// The terminal: a VT parser and the state it drives.
pub struct Terminal {
    parser: vte::Parser,
    splitter: Splitter,
    state: State,
}

/// The cursor as `DECSC` saves it.
///
/// Law: `Copy` is sound because a saved cursor is a value.
#[derive(Clone, Copy, Debug)]
struct Saved {
    col: u16,
    row: u16,
    pen: Style,
    wrap_pending: bool,
    origin: bool,
    charsets: Charsets,
}

/// Which character set shows as which.
///
/// Law: `Copy` is sound because charsets are tags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Charsets {
    g0: Charset,
    g1: Charset,
    /// Whether G1 is in use (after SO).
    shifted: bool,
}

/// A character set.
///
/// Law: `Copy` is sound because a charset is a tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Charset {
    #[default]
    Ascii,
    /// DEC special graphics: line drawing.
    Graphics,
}

/// Everything the VT parser changes.
struct State {
    screen: Screen,
    /// The main screen while the alternate one shows.
    main: Option<Screen>,
    pen: Style,
    saved: Option<Saved>,
    wrap_pending: bool,
    top: u16,
    bottom: u16,
    modes: Modes,
    charsets: Charsets,
    last: Option<char>,
    images: graphics::Images,
    options: Options,
    replies: Vec<u8>,
    events: Vec<Event>,
}

impl Terminal {
    /// A blank terminal of `size`.
    #[must_use]
    pub fn new(size: Size, options: Options) -> Self {
        Self {
            parser: vte::Parser::new(),
            splitter: Splitter::default(),
            state: State::new(size, options),
        }
    }

    /// Reads what Emacs wrote.
    pub fn feed(&mut self, bytes: &[u8]) {
        for segment in self.splitter.split(bytes) {
            match segment {
                Segment::Pass(bytes) => self.parser.advance(&mut self.state, &bytes),
                Segment::Apc(payload) => self.state.graphics(&payload),
            }
        }
    }

    /// The screen as it stands.
    #[must_use]
    pub const fn screen(&self) -> &Screen {
        &self.state.screen
    }

    /// The modes as they stand.
    #[must_use]
    pub const fn modes(&self) -> &Modes {
        &self.state.modes
    }

    /// The options in force.
    #[must_use]
    pub const fn options(&self) -> &Options {
        &self.state.options
    }

    /// Changes the size, as a resized window would.
    pub fn resize(&mut self, size: Size) {
        self.state.resize(size);
    }

    /// Changes what the terminal says about the client.
    pub fn set_options(&mut self, options: Options) {
        self.state.options = options;
    }

    /// The answers waiting for Emacs, oldest first.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.state.replies)
    }

    /// The events since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.state.events)
    }
}

impl State {
    fn new(size: Size, options: Options) -> Self {
        Self {
            screen: Screen::blank(size),
            main: None,
            pen: Style::PLAIN,
            saved: None,
            wrap_pending: false,
            top: 0,
            bottom: size.last_row(),
            modes: Modes::default(),
            charsets: Charsets::default(),
            last: None,
            images: graphics::Images::default(),
            options,
            replies: Vec::new(),
            events: Vec::new(),
        }
    }

    fn reply(&mut self, bytes: &[u8]) {
        self.replies.extend_from_slice(bytes);
    }
}
