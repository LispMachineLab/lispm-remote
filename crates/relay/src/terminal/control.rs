// lispm-remote: control functions, as the VT parser dispatches them.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The terminal's answer to each control function the parser finds:
//! C0 controls, `ESC` sequences, CSI sequences, OSC strings and DCS
//! requests.  Functions Emacs never sends, and unknown ones, are ignored
//! as an xterm ignores them.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lispm_remote_display::CursorShape;
use lispm_remote_display::Rgb;
use vte::Params;
use vte::Perform;

use super::Charset;
use super::Event;
use super::MouseMode;
use super::State;
use super::grid::Extent;
use super::sgr;

/// Primary device attributes: a VT220 with colour and clipboard writes.
const PRIMARY_ATTRIBUTES: &[u8] = b"\x1b[?62;22;52c";
/// Secondary device attributes: xterm patch 390, which is what makes
/// Emacs's xterm.el turn on modifyOtherKeys, OSC 52 and colour queries.
const SECONDARY_ATTRIBUTES: &[u8] = b"\x1b[>41;390;0c";
/// The answer to XTVERSION.
const VERSION: &[u8] = b"\x1bP>|LispM Remote\x1b\\";
/// The answer to XTGETTCAP: no capability is known.
const NO_CAPABILITY: &[u8] = b"\x1bP0+r\x1b\\";

/// Parameter `index`, or `default` when absent or zero.
fn arg(params: &Params, index: usize, default: u16) -> u16 {
    raw(params, index)
        .filter(|&value| value != 0)
        .unwrap_or(default)
}

/// Parameter `index` as written, if it was.
fn raw(params: &Params, index: usize) -> Option<u16> {
    params
        .iter()
        .nth(index)
        .and_then(|param| param.first().copied())
}

impl Perform for State {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.events.push(Event::Bell),
            0x08 => self.backspace(),
            0x09 => self.tab(1, false),
            0x0a..=0x0c => self.line_feed(),
            0x0d => self.carriage_return(),
            0x0e => self.charsets.shifted = true,
            0x0f => self.charsets.shifted = false,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if !ignore {
            self.csi(params, intermediates, action);
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.line_feed(),
            ([], b'E') => {
                self.carriage_return();
                self.line_feed();
            }
            ([], b'M') => self.reverse_index(),
            ([], b'c') => self.reset(),
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([b'('], set) => self.charsets.g0 = charset(set),
            ([b')'], set) => self.charsets.g1 = charset(set),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let end: &[u8] = if bell_terminated { b"\x07" } else { b"\x1b\\" };
        match params {
            [b"0" | b"2", title @ ..] => {
                let title = title.join(&b';');
                self.screen.title = Some(String::from_utf8_lossy(&title).into_owned());
            }
            [b"52", _, data, ..] if *data != b"?" => {
                if let Some(text) = STANDARD
                    .decode(data)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                {
                    self.events.push(Event::Clipboard(text));
                }
            }
            [b"10", b"?"] => self.reply_color(10, self.options.foreground, end),
            [b"11", b"?"] => self.reply_color(11, self.options.background, end),
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if !ignore && intermediates == b"+" && action == 'q' {
            self.reply(NO_CAPABILITY);
        }
    }
}

const fn charset(set: u8) -> Charset {
    if set == b'0' {
        Charset::Graphics
    } else {
        Charset::Ascii
    }
}

impl State {
    fn reply_color(&mut self, which: u8, color: Rgb, end: &[u8]) {
        let channel = |value: u8| format!("{value:02x}{value:02x}");
        let answer = format!(
            "\x1b]{which};rgb:{}/{}/{}",
            channel(color.r),
            channel(color.g),
            channel(color.b)
        );
        self.reply(answer.as_bytes());
        self.reply(end);
    }

    fn csi(&mut self, params: &Params, intermediates: &[u8], action: char) {
        let n = arg(params, 0, 1);
        let signed = i32::from(n);
        match (intermediates, action) {
            ([], '@') => self.insert_chars(n),
            ([], 'A') => self.move_by(0, -signed),
            ([], 'B' | 'e') => self.move_by(0, signed),
            ([], 'C' | 'a') => self.move_by(signed, 0),
            ([], 'D') => self.move_by(-signed, 0),
            ([], 'E') => {
                self.move_by(0, signed);
                self.carriage_return();
            }
            ([], 'F') => {
                self.move_by(0, -signed);
                self.carriage_return();
            }
            ([], 'G' | '`') => self.move_to_col(n - 1),
            ([], 'H' | 'f') => self.move_to(arg(params, 1, 1) - 1, n - 1),
            ([], 'I') => self.tab(n, false),
            ([], 'Z') => self.tab(n, true),
            ([] | [b'?'], 'J') => {
                if let Some(extent) = Extent::of(raw(params, 0).unwrap_or(0)) {
                    self.erase_in_display(extent);
                }
            }
            ([] | [b'?'], 'K') => {
                if let Some(extent) = Extent::of(raw(params, 0).unwrap_or(0)) {
                    self.erase_in_line(extent);
                }
            }
            ([], 'L') => self.insert_lines(n),
            ([], 'M') => self.delete_lines(n),
            ([], 'P') => self.delete_chars(n),
            ([], 'S') => self.scroll_up(n),
            ([], 'T') => self.scroll_down(n),
            ([], 'X') => self.erase_chars(n),
            ([], 'b') => self.repeat(n),
            ([], 'c') if raw(params, 0).unwrap_or(0) == 0 => self.reply(PRIMARY_ATTRIBUTES),
            ([b'>'], 'c') => self.reply(SECONDARY_ATTRIBUTES),
            ([], 'd') => self.move_to_row(n - 1),
            ([], 'h') => self.ansi_modes(params, true),
            ([], 'l') => self.ansi_modes(params, false),
            ([b'?'], 'h') => self.private_modes(params, true),
            ([b'?'], 'l') => self.private_modes(params, false),
            ([], 'm') => self.pen = sgr::apply(self.pen, params),
            ([b'>'], 'm') => {
                if raw(params, 0) == Some(4) {
                    self.modes.modify_other_keys = raw(params, 1)
                        .and_then(|level| u8::try_from(level).ok())
                        .unwrap_or(0);
                }
            }
            ([], 'n') => self.status_report(raw(params, 0).unwrap_or(0)),
            ([b'?', b'$'], 'p') => self.report_mode(raw(params, 0).unwrap_or(0)),
            ([b' '], 'q') => self.screen.cursor.shape = cursor_shape(raw(params, 0).unwrap_or(0)),
            ([b'>'], 'q') => self.reply(VERSION),
            ([], 'r') => {
                let bottom = arg(params, 1, self.size().rows());
                self.set_region(n - 1, bottom - 1);
            }
            ([], 's') => self.save_cursor(),
            ([], 'u') => self.restore_cursor(),
            ([], 't') => self.window_report(raw(params, 0).unwrap_or(0)),
            _ => {}
        }
    }

    fn ansi_modes(&mut self, params: &Params, on: bool) {
        for mode in params.iter().filter_map(|param| param.first().copied()) {
            if mode == 4 {
                self.modes.insert = on;
            }
        }
    }

    fn private_modes(&mut self, params: &Params, on: bool) {
        for mode in params.iter().filter_map(|param| param.first().copied()) {
            match mode {
                1 => self.modes.app_cursor = on,
                6 => {
                    self.modes.origin = on;
                    self.move_to(0, 0);
                }
                7 => self.modes.autowrap = on,
                25 => self.screen.cursor.visible = on,
                47 | 1047 => self.alternate(on, false),
                1048 => {
                    if on {
                        self.save_cursor();
                    } else {
                        self.restore_cursor();
                    }
                }
                1049 => self.alternate(on, true),
                1000 => self.mouse(MouseMode::Press, on),
                1002 => self.mouse(MouseMode::Drag, on),
                1003 => self.mouse(MouseMode::Any, on),
                1004 => self.modes.focus = on,
                1006 => self.modes.sgr_mouse = on,
                2004 => self.modes.bracketed_paste = on,
                2026 => self.modes.synchronized = on,
                _ => {}
            }
        }
    }

    fn mouse(&mut self, mode: MouseMode, on: bool) {
        if on {
            self.modes.mouse = mode;
        } else if self.modes.mouse == mode {
            self.modes.mouse = MouseMode::Off;
        }
    }

    /// The state of private mode `mode`, as DECRQM reports it: 1 set,
    /// 2 reset, 0 unknown.
    fn mode_state(&self, mode: u16) -> u8 {
        let modes = &self.modes;
        let set = match mode {
            1 => modes.app_cursor,
            6 => modes.origin,
            7 => modes.autowrap,
            25 => self.screen.cursor.visible,
            1049 => self.main.is_some(),
            1000 => modes.mouse == MouseMode::Press,
            1002 => modes.mouse == MouseMode::Drag,
            1003 => modes.mouse == MouseMode::Any,
            1004 => modes.focus,
            1006 => modes.sgr_mouse,
            2004 => modes.bracketed_paste,
            2026 => modes.synchronized,
            _ => return 0,
        };
        if set { 1 } else { 2 }
    }

    fn report_mode(&mut self, mode: u16) {
        let answer = format!("\x1b[?{mode};{}$y", self.mode_state(mode));
        self.reply(answer.as_bytes());
    }

    fn status_report(&mut self, which: u16) {
        match which {
            5 => self.reply(b"\x1b[0n"),
            6 => {
                let cursor = self.screen.cursor;
                let row = if self.modes.origin {
                    cursor.row.saturating_sub(self.top)
                } else {
                    cursor.row
                };
                let answer = format!("\x1b[{};{}R", u32::from(row) + 1, u32::from(cursor.col) + 1);
                self.reply(answer.as_bytes());
            }
            _ => {}
        }
    }

    fn window_report(&mut self, which: u16) {
        let size = self.size();
        let cell = self.options.cell;
        let answer = match which {
            14 => format!(
                "\x1b[4;{};{}t",
                u32::from(size.rows()) * u32::from(cell.height),
                u32::from(size.cols()) * u32::from(cell.width)
            ),
            16 => format!("\x1b[6;{};{}t", cell.height, cell.width),
            18 => format!("\x1b[8;{};{}t", size.rows(), size.cols()),
            _ => return,
        };
        self.reply(answer.as_bytes());
    }
}

const fn cursor_shape(style: u16) -> CursorShape {
    match style {
        3 | 4 => CursorShape::Underline,
        5 | 6 => CursorShape::Bar,
        _ => CursorShape::Block,
    }
}
