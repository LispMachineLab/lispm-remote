// lispm-remote: what the terminal's commands do to the grid.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The grid's operations, as an xterm performs them: printing with
//! pending wrap, wide characters and combining marks; erasing in the
//! pen's background (background colour erase); scrolling inside the
//! region; the cursor's moves, saved cursor and alternate screen.
//!
//! Invariant: the cursor is on the grid, `top ≤ bottom < rows`, and no
//! wide character is split: a spacer always follows a wide character's
//! cell, and only there.

use std::sync::Arc;

use lispm_remote_display::Cell;
use lispm_remote_display::Content;
use lispm_remote_display::Screen;
use lispm_remote_display::Size;
use lispm_remote_display::Style;
use unicode_width::UnicodeWidthChar;

use super::Charset;
use super::Charsets;
use super::Saved;
use super::State;

/// The longest cluster kept, in bytes; marks past it are dropped.
const MAX_CLUSTER: usize = 32;
/// Tab stops are every this many columns.
const TAB: u16 = 8;

/// What part of a line or of the screen an erase covers.
///
/// Law: `Copy` is sound because the extent is a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Extent {
    /// From the cursor on.
    After,
    /// Up to and with the cursor.
    Before,
    /// All of it.
    All,
}

impl Extent {
    pub(super) const fn of(param: u16) -> Option<Self> {
        match param {
            0 => Some(Self::After),
            1 => Some(Self::Before),
            2 => Some(Self::All),
            _ => None,
        }
    }
}

impl State {
    pub(super) const fn size(&self) -> Size {
        self.screen.size()
    }

    fn col(&self) -> u16 {
        self.screen.cursor.col
    }

    fn row(&self) -> u16 {
        self.screen.cursor.row
    }

    /// Prints `c` at the cursor and moves past it.
    pub(super) fn put_char(&mut self, c: char) {
        let c = self.translate(c);
        let width = c.width().unwrap_or(1);
        if width == 0 {
            self.combine(c);
            return;
        }
        let wide = width >= 2;
        if self.wrap_pending && self.modes.autowrap {
            self.carriage_return();
            self.line_feed();
        }
        self.wrap_pending = false;
        if wide && self.col() == self.size().last_col() {
            if !self.modes.autowrap {
                return;
            }
            self.carriage_return();
            self.line_feed();
        }
        let span = if wide { 2 } else { 1 };
        if self.modes.insert {
            self.insert_chars(span);
        }
        let (col, row) = (self.col(), self.row());
        let style = self.pen;
        if let Some(line) = self.screen.row_mut(row) {
            let cells = line.cells_mut();
            let at = usize::from(col);
            cut_left(cells, at, style);
            cut_right(cells, at + usize::from(span), style);
            if let Some(cell) = cells.get_mut(at) {
                *cell = Cell {
                    content: Content::Char(c),
                    style,
                };
            }
            if wide {
                if let Some(cell) = cells.get_mut(at + 1) {
                    *cell = Cell {
                        content: Content::Spacer,
                        style,
                    };
                }
            }
        }
        self.last = Some(c);
        let next = col.saturating_add(span);
        if next > self.size().last_col() {
            self.screen.cursor.col = self.size().last_col();
            self.wrap_pending = self.modes.autowrap;
        } else {
            self.screen.cursor.col = next;
        }
    }

    /// The character `c` stands for in the charset in use.
    fn translate(&self, c: char) -> char {
        let Charsets { g0, g1, shifted } = self.charsets;
        match if shifted { g1 } else { g0 } {
            Charset::Ascii => c,
            Charset::Graphics => dec_graphics(c),
        }
    }

    /// Adds the combining character `c` to the cell before the cursor.
    fn combine(&mut self, c: char) {
        let (col, row) = (self.col(), self.row());
        let mut at = if self.wrap_pending {
            col
        } else {
            col.saturating_sub(1)
        };
        if let Some(line) = self.screen.row_mut(row) {
            let cells = line.cells_mut();
            if at > 0
                && cells
                    .get(usize::from(at))
                    .is_some_and(|cell| cell.content.is_spacer())
            {
                at -= 1;
            }
            if let Some(cell) = cells.get_mut(usize::from(at)) {
                let joined = match &cell.content {
                    Content::Char(base) => Some(format!("{base}{c}")),
                    Content::Cluster(text) if text.len() + c.len_utf8() <= MAX_CLUSTER => {
                        Some(format!("{text}{c}"))
                    }
                    Content::Cluster(_) | Content::Spacer => None,
                };
                if let Some(text) = joined {
                    cell.content = Content::Cluster(Arc::from(text));
                }
            }
        }
    }

    /// Repeats the last printed character `n` times (REP).
    pub(super) fn repeat(&mut self, n: u16) {
        if let Some(c) = self.last {
            for _ in 0..n.min(self.size().cols()) {
                self.put_char(c);
            }
        }
    }

    pub(super) const fn carriage_return(&mut self) {
        self.screen.cursor.col = 0;
        self.wrap_pending = false;
    }

    /// Moves down a line, scrolling the region at its bottom (IND, LF).
    pub(super) fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.row() == self.bottom {
            self.screen
                .scroll_up(self.top, self.bottom, 1, self.pen.erased());
        } else if self.row() < self.size().last_row() {
            self.screen.cursor.row += 1;
        }
    }

    /// Moves up a line, scrolling the region at its top (RI).
    pub(super) fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.row() == self.top {
            self.screen
                .scroll_down(self.top, self.bottom, 1, self.pen.erased());
        } else if self.row() > 0 {
            self.screen.cursor.row -= 1;
        }
    }

    pub(super) const fn backspace(&mut self) {
        self.wrap_pending = false;
        self.screen.cursor.col = self.screen.cursor.col.saturating_sub(1);
    }

    /// Moves to the `n`th tab stop right (or left, when `back`).
    pub(super) fn tab(&mut self, n: u16, back: bool) {
        self.wrap_pending = false;
        let mut col = self.col();
        for _ in 0..n {
            col = if back {
                col.saturating_sub(1) / TAB * TAB
            } else {
                (col / TAB + 1)
                    .saturating_mul(TAB)
                    .min(self.size().last_col())
            };
        }
        self.screen.cursor.col = col;
    }

    /// Moves to column `col` and row `row` counted from 0, rows from the
    /// region's top in origin mode; clamped to the grid (or region).
    pub(super) fn move_to(&mut self, col: u16, row: u16) {
        self.wrap_pending = false;
        let (top, bottom) = if self.modes.origin {
            (self.top, self.bottom)
        } else {
            (0, self.size().last_row())
        };
        self.screen.cursor.col = col.min(self.size().last_col());
        self.screen.cursor.row = top.saturating_add(row).min(bottom);
    }

    /// Moves to column `col`, the row unchanged.
    pub(super) fn move_to_col(&mut self, col: u16) {
        self.wrap_pending = false;
        self.screen.cursor.col = col.min(self.size().last_col());
    }

    /// Moves to row `row` (as [`State::move_to`] counts), the column
    /// unchanged.
    pub(super) fn move_to_row(&mut self, row: u16) {
        let col = self.col();
        self.move_to(col, row);
        self.screen.cursor.col = col;
    }

    /// Moves `rows` up or down and `cols` left or right, stopping at the
    /// region's margins when inside it, else at the grid's edges.
    pub(super) fn move_by(&mut self, cols: i32, rows: i32) {
        self.wrap_pending = false;
        let row = i32::from(self.row());
        let inside = row >= i32::from(self.top) && row <= i32::from(self.bottom);
        let (top, bottom) = if inside {
            (i32::from(self.top), i32::from(self.bottom))
        } else {
            (0, i32::from(self.size().last_row()))
        };
        let col = (i32::from(self.col()) + cols).clamp(0, i32::from(self.size().last_col()));
        let row = (row + rows).clamp(top, bottom);
        self.screen.cursor.col = u16::try_from(col).unwrap_or(0);
        self.screen.cursor.row = u16::try_from(row).unwrap_or(0);
    }

    /// Erases cells `from..to` of `row` in the pen's background.
    fn erase(&mut self, row: u16, from: u16, to: u16) {
        let style = self.pen.erased();
        if let Some(line) = self.screen.row_mut(row) {
            let cells = line.cells_mut();
            cut_left(cells, usize::from(from), style);
            cut_right(cells, usize::from(to), style);
            let range = usize::from(from)..usize::from(to).min(cells.len());
            if let Some(span) = cells.get_mut(range) {
                span.fill(Cell::blank(style));
            }
        }
    }

    /// Erases part of the cursor's line (EL).
    pub(super) fn erase_in_line(&mut self, extent: super::grid::Extent) {
        let (col, row, cols) = (self.col(), self.row(), self.size().cols());
        match extent {
            Extent::After => self.erase(row, col, cols),
            Extent::Before => self.erase(row, 0, col.saturating_add(1)),
            Extent::All => self.erase(row, 0, cols),
        }
    }

    /// Erases part of the screen (ED); erasing all of it takes its
    /// images away too.
    pub(super) fn erase_in_display(&mut self, extent: Extent) {
        let (row, rows, cols) = (self.row(), self.size().rows(), self.size().cols());
        match extent {
            Extent::After => {
                self.erase_in_line(Extent::After);
                for below in row.saturating_add(1)..rows {
                    self.erase(below, 0, cols);
                }
            }
            Extent::Before => {
                for above in 0..row {
                    self.erase(above, 0, cols);
                }
                self.erase_in_line(Extent::Before);
            }
            Extent::All => {
                for any in 0..rows {
                    self.erase(any, 0, cols);
                }
                self.screen.placements.clear();
            }
        }
    }

    /// Erases `n` cells from the cursor on (ECH).
    pub(super) fn erase_chars(&mut self, n: u16) {
        let (col, row) = (self.col(), self.row());
        self.erase(row, col, col.saturating_add(n));
    }

    /// Inserts `n` blank cells at the cursor, pushing the rest right
    /// (ICH).
    pub(super) fn insert_chars(&mut self, n: u16) {
        self.shift_cells(n, true);
    }

    /// Deletes `n` cells at the cursor, pulling the rest left (DCH).
    pub(super) fn delete_chars(&mut self, n: u16) {
        self.shift_cells(n, false);
    }

    fn shift_cells(&mut self, n: u16, right: bool) {
        let (col, row) = (usize::from(self.col()), self.row());
        let style = self.pen.erased();
        self.wrap_pending = false;
        if let Some(line) = self.screen.row_mut(row) {
            let cells = line.cells_mut();
            cut_left(cells, col, style);
            if !right {
                cut_right(cells, col + usize::from(n), style);
            }
            if let Some(tail) = cells.get_mut(col..) {
                let n = usize::from(n).min(tail.len());
                let len = tail.len();
                if right {
                    tail.rotate_right(n);
                    tail.iter_mut()
                        .take(n)
                        .for_each(|cell| *cell = Cell::blank(style));
                } else {
                    tail.rotate_left(n);
                    tail.iter_mut()
                        .skip(len - n)
                        .for_each(|cell| *cell = Cell::blank(style));
                }
            }
            if right {
                if let Some(last) = cells.last_mut() {
                    if wide_head(last) {
                        *last = Cell::blank(style);
                    }
                }
            }
        }
    }

    /// Inserts `n` blank lines at the cursor, inside the region (IL).
    pub(super) fn insert_lines(&mut self, n: u16) {
        if self.in_region() {
            self.screen
                .scroll_down(self.row(), self.bottom, n, self.pen.erased());
            self.carriage_return();
        }
    }

    /// Deletes `n` lines at the cursor, inside the region (DL).
    pub(super) fn delete_lines(&mut self, n: u16) {
        if self.in_region() {
            self.screen
                .scroll_up(self.row(), self.bottom, n, self.pen.erased());
            self.carriage_return();
        }
    }

    fn in_region(&self) -> bool {
        (self.top..=self.bottom).contains(&self.row())
    }

    /// Scrolls the region up `n` lines (SU).
    pub(super) fn scroll_up(&mut self, n: u16) {
        self.screen
            .scroll_up(self.top, self.bottom, n, self.pen.erased());
    }

    /// Scrolls the region down `n` lines (SD).
    pub(super) fn scroll_down(&mut self, n: u16) {
        self.screen
            .scroll_down(self.top, self.bottom, n, self.pen.erased());
    }

    /// Sets the scrolling region to rows `top..=bottom`, counted from 0,
    /// and homes the cursor (DECSTBM).  A region of one row is refused.
    pub(super) fn set_region(&mut self, top: u16, bottom: u16) {
        let bottom = bottom.min(self.size().last_row());
        if top < bottom {
            self.top = top;
            self.bottom = bottom;
            self.move_to(0, 0);
        }
    }

    pub(super) fn save_cursor(&mut self) {
        self.saved = Some(Saved {
            col: self.col(),
            row: self.row(),
            pen: self.pen,
            wrap_pending: self.wrap_pending,
            origin: self.modes.origin,
            charsets: self.charsets,
        });
    }

    pub(super) fn restore_cursor(&mut self) {
        let saved = self.saved.unwrap_or(Saved {
            col: 0,
            row: 0,
            pen: Style::PLAIN,
            wrap_pending: false,
            origin: false,
            charsets: Charsets::default(),
        });
        self.screen.cursor.col = saved.col.min(self.size().last_col());
        self.screen.cursor.row = saved.row.min(self.size().last_row());
        self.pen = saved.pen;
        self.wrap_pending = saved.wrap_pending;
        self.modes.origin = saved.origin;
        self.charsets = saved.charsets;
    }

    /// Shows the alternate screen (`on`) or the main one again; `save`
    /// also saves and restores the cursor, as mode 1049 does.
    pub(super) fn alternate(&mut self, on: bool, save: bool) {
        match (on, self.main.is_some()) {
            (true, false) => {
                if save {
                    self.save_cursor();
                }
                let mut alternate = Screen::blank(self.size());
                alternate.cursor = self.screen.cursor;
                let main = std::mem::replace(&mut self.screen, alternate);
                self.main = Some(main);
            }
            (false, true) => {
                if let Some(main) = self.main.take() {
                    let cursor = self.screen.cursor;
                    self.screen = main.resized(self.size());
                    self.screen.cursor = cursor;
                }
                if save {
                    self.restore_cursor();
                }
            }
            _ => {}
        }
        self.wrap_pending = false;
    }

    /// Takes a new size: content kept at the top left, the region reset.
    pub(super) fn resize(&mut self, size: Size) {
        self.screen = self.screen.resized(size);
        self.main = self.main.as_ref().map(|main| main.resized(size));
        self.top = 0;
        self.bottom = size.last_row();
        self.wrap_pending = false;
    }

    /// Returns to the state at power on, keeping the size and options
    /// (RIS).
    pub(super) fn reset(&mut self) {
        let images = std::mem::take(&mut self.images);
        *self = Self::new(self.size(), self.options);
        self.images = images;
    }
}

/// Before the cells from `col` on change: a wide character cut at
/// `col` (its spacer there) is blanked, both halves, in `style`.
fn cut_left(cells: &mut [Cell], col: usize, style: Style) {
    if cells.get(col).is_some_and(|cell| cell.content.is_spacer()) {
        if let Some(head) = col.checked_sub(1).and_then(|left| cells.get_mut(left)) {
            *head = Cell::blank(style);
        }
        if let Some(spacer) = cells.get_mut(col) {
            *spacer = Cell::blank(style);
        }
    }
}

/// Before the cells before `end` change: a spacer at `end`, whose wide
/// character is among them, is blanked in `style`.
fn cut_right(cells: &mut [Cell], end: usize, style: Style) {
    if let Some(spacer) = cells.get_mut(end) {
        if spacer.content.is_spacer() {
            *spacer = Cell::blank(style);
        }
    }
}

/// Whether `cell` holds a character two cells wide.
fn wide_head(cell: &Cell) -> bool {
    let first = match &cell.content {
        Content::Char(c) => Some(*c),
        Content::Cluster(text) => text.chars().next(),
        Content::Spacer => None,
    };
    first.and_then(UnicodeWidthChar::width) == Some(2)
}

/// DEC special graphics: the line-drawing set `ESC ( 0` selects.
const fn dec_graphics(c: char) -> char {
    match c {
        '`' => '\u{25c6}',
        'a' => '\u{2592}',
        'f' => '\u{b0}',
        'g' => '\u{b1}',
        'j' => '\u{2518}',
        'k' => '\u{2510}',
        'l' => '\u{250c}',
        'm' => '\u{2514}',
        'n' => '\u{253c}',
        'o' => '\u{23ba}',
        'p' => '\u{23bb}',
        'q' => '\u{2500}',
        'r' => '\u{23bc}',
        's' => '\u{23bd}',
        't' => '\u{251c}',
        'u' => '\u{2524}',
        'v' => '\u{2534}',
        'w' => '\u{252c}',
        'x' => '\u{2502}',
        'y' => '\u{2264}',
        'z' => '\u{2265}',
        '{' => '\u{3c0}',
        '|' => '\u{2260}',
        '}' => '\u{a3}',
        '~' => '\u{b7}',
        other => other,
    }
}
