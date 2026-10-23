// lispm-remote: the screen of one surface.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A screen: a grid of cells, the cursor, the title, and the images
//! placed over the grid.
//!
//! Invariant: `rows.len() = size.rows` and every row is `size.cols`
//! cells long.  [`Screen::blank`] establishes it; every method that
//! changes rows keeps it.

use std::sync::Arc;

use crate::cell::Row;
use crate::resource::ResourceId;
use crate::style::Style;

/// The largest number of columns or rows a screen has.
pub const MAX_CELLS: u16 = 1000;

/// A screen's size in cells.
///
/// Law: `Copy` is sound because a size is a pair of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Size {
    cols: u16,
    rows: u16,
}

/// Why a size cannot be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SizeError {
    /// A dimension is zero or larger than [`MAX_CELLS`].
    #[error("{cols}×{rows} cells is not a screen size")]
    OutOfRange {
        /// The columns asked for.
        cols: u16,
        /// The rows asked for.
        rows: u16,
    },
    /// A row is not as long as the screen is wide.
    #[error("a row of {len} cells on a screen {cols} wide")]
    RowLength {
        /// The row's length.
        len: usize,
        /// The screen's width.
        cols: u16,
    },
}

impl Size {
    /// The size `cols × rows`, both in `1..=MAX_CELLS`.
    ///
    /// # Errors
    ///
    /// [`SizeError::OutOfRange`] otherwise.
    pub const fn new(cols: u16, rows: u16) -> Result<Self, SizeError> {
        if cols == 0 || rows == 0 || cols > MAX_CELLS || rows > MAX_CELLS {
            Err(SizeError::OutOfRange { cols, rows })
        } else {
            Ok(Self { cols, rows })
        }
    }

    /// The width.
    #[must_use]
    pub const fn cols(self) -> u16 {
        self.cols
    }

    /// The height.
    #[must_use]
    pub const fn rows(self) -> u16 {
        self.rows
    }

    /// The last column.
    #[must_use]
    pub const fn last_col(self) -> u16 {
        self.cols - 1
    }

    /// The last row.
    #[must_use]
    pub const fn last_row(self) -> u16 {
        self.rows - 1
    }
}

/// How the cursor is drawn.
///
/// Law: `Copy` is sound because the shape is a tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    /// A filled cell.
    #[default]
    Block,
    /// A line under the cell.
    Underline,
    /// A line at the cell's left edge.
    Bar,
}

/// Where the cursor is, and whether it shows.
///
/// Law: `Copy` is sound because a cursor is a value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cursor {
    /// Its column.
    pub col: u16,
    /// Its row.
    pub row: u16,
    /// Whether it is drawn.
    pub visible: bool,
    /// How it is drawn.
    pub shape: CursorShape,
}

/// A rectangle of cells.
///
/// Law: `Copy` is sound because a rectangle is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRect {
    /// The left column.
    pub col: u16,
    /// The top row.
    pub row: u16,
    /// How many columns.
    pub cols: u16,
    /// How many rows.
    pub rows: u16,
}

/// A rectangle of an image's pixels.
///
/// Law: `Copy` is sound because a rectangle is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelRect {
    /// The left edge.
    pub x: u32,
    /// The top edge.
    pub y: u32,
    /// The width.
    pub w: u32,
    /// The height.
    pub h: u32,
}

/// What names a placement: its image's number and its own, as the
/// program that placed it chose them.
///
/// Law: `Copy` is sound because a key is a pair of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlacementKey {
    /// The image's number.
    pub image: u32,
    /// The placement's number among the image's.
    pub placement: u32,
}

/// An image drawn over cells.
///
/// Law: `Copy` is sound because a placement is a value; its image is
/// named, not held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    /// What names it.
    pub key: PlacementKey,
    /// The image.
    pub resource: ResourceId,
    /// The cells it covers.
    pub cells: CellRect,
    /// The part of the image shown, or all of it.
    pub crop: Option<PixelRect>,
    /// How many pixels right of the first cell's edge it starts.
    pub offset_x: u16,
    /// How many pixels below the first cell's edge it starts.
    pub offset_y: u16,
    /// Its layer: below the text when negative.
    pub z: i32,
}

/// One surface's screen.
///
/// Law: `Clone` shares rows (`Arc`), so a clone is cheap and a snapshot;
/// changing either copy copies only the changed rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    size: Size,
    rows: Vec<Arc<Row>>,
    /// The cursor.
    pub cursor: Cursor,
    /// The title, as the program last set it.
    pub title: Option<String>,
    /// The images, in no particular order; keys are unique.
    pub placements: Vec<Placement>,
}

impl Screen {
    /// A blank screen of `size`, the cursor at the top left.
    #[must_use]
    pub fn blank(size: Size) -> Self {
        let row = Arc::new(Row::blank(size.cols, Style::PLAIN));
        Self {
            size,
            rows: vec![row; usize::from(size.rows)],
            cursor: Cursor {
                visible: true,
                ..Cursor::default()
            },
            title: None,
            placements: Vec::new(),
        }
    }

    /// The screen of `size` holding `rows`.
    ///
    /// # Errors
    ///
    /// [`SizeError`] when there are not `size.rows` rows of `size.cols`
    /// cells.
    pub fn from_rows(size: Size, rows: Vec<Arc<Row>>) -> Result<Self, SizeError> {
        if rows.len() != usize::from(size.rows) {
            return Err(SizeError::OutOfRange {
                cols: size.cols,
                rows: u16::try_from(rows.len()).unwrap_or(u16::MAX),
            });
        }
        if let Some(row) = rows.iter().find(|row| !row.fits(size.cols)) {
            return Err(SizeError::RowLength {
                len: row.len(),
                cols: size.cols,
            });
        }
        Ok(Self {
            rows,
            ..Self::blank(size)
        })
    }

    /// The size.
    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// The rows, top to bottom.
    #[must_use]
    pub fn rows(&self) -> &[Arc<Row>] {
        &self.rows
    }

    /// Row `index`, if the screen has it.
    #[must_use]
    pub fn row(&self, index: u16) -> Option<&Row> {
        self.rows.get(usize::from(index)).map(AsRef::as_ref)
    }

    /// Row `index`, to change in place (copied first if shared).
    pub fn row_mut(&mut self, index: u16) -> Option<&mut Row> {
        self.rows.get_mut(usize::from(index)).map(Arc::make_mut)
    }

    /// Puts `row` at `index`.
    ///
    /// # Errors
    ///
    /// [`SizeError::RowLength`] when `row` is not as wide as the screen;
    /// an index past the bottom changes nothing.
    pub fn set_row(&mut self, index: u16, row: Arc<Row>) -> Result<(), SizeError> {
        if !row.fits(self.size.cols) {
            return Err(SizeError::RowLength {
                len: row.len(),
                cols: self.size.cols,
            });
        }
        if let Some(slot) = self.rows.get_mut(usize::from(index)) {
            *slot = row;
        }
        Ok(())
    }

    /// Scrolls rows `top..=bottom` up by `n`: row `top + n` moves to
    /// `top`, and `n` rows blank in `style` enter at the bottom.
    ///
    /// Pre: `top ≤ bottom < size.rows`; otherwise nothing changes.
    pub fn scroll_up(&mut self, top: u16, bottom: u16, n: u16, style: Style) {
        let blank = Arc::new(Row::blank(self.size.cols, style));
        if let Some(region) = self.region(top, bottom) {
            let n = usize::from(n).min(region.len());
            region.rotate_left(n);
            let len = region.len();
            for row in region.iter_mut().skip(len - n) {
                *row = Arc::clone(&blank);
            }
        }
    }

    /// Scrolls rows `top..=bottom` down by `n`, blank rows in `style`
    /// entering at the top.  Pre: as [`Screen::scroll_up`].
    pub fn scroll_down(&mut self, top: u16, bottom: u16, n: u16, style: Style) {
        let blank = Arc::new(Row::blank(self.size.cols, style));
        if let Some(region) = self.region(top, bottom) {
            let n = usize::from(n).min(region.len());
            region.rotate_right(n);
            for row in region.iter_mut().take(n) {
                *row = Arc::clone(&blank);
            }
        }
    }

    fn region(&mut self, top: u16, bottom: u16) -> Option<&mut [Arc<Row>]> {
        if top > bottom {
            return None;
        }
        self.rows.get_mut(usize::from(top)..=usize::from(bottom))
    }

    /// This screen at `size`: the top left of its cells kept, the rest
    /// blank; the cursor kept inside.
    #[must_use]
    pub fn resized(&self, size: Size) -> Self {
        let mut next = Self::blank(size);
        for (slot, row) in next.rows.iter_mut().zip(&self.rows) {
            if row.fits(size.cols) {
                *slot = Arc::clone(row);
            } else {
                let mut cells = Row::blank(size.cols, Style::PLAIN);
                for (cell, old) in cells.cells_mut().iter_mut().zip(row.cells()) {
                    cell.clone_from(old);
                }
                *slot = Arc::new(cells);
            }
        }
        next.cursor = Cursor {
            col: self.cursor.col.min(size.last_col()),
            row: self.cursor.row.min(size.last_row()),
            ..self.cursor
        };
        next.title.clone_from(&self.title);
        next.placements.clone_from(&self.placements);
        next
    }

    /// The screen's text, one line per row, spacers left out.
    #[must_use]
    pub fn text(&self) -> String {
        let lines: Vec<String> = self.rows.iter().map(|row| row.text()).collect();
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::cell::Content;

    fn lettered(size: Size) -> Screen {
        let mut screen = Screen::blank(size);
        for (index, letter) in (0..size.rows()).zip('a'..) {
            let mut row = Row::blank(size.cols(), Style::PLAIN);
            if let Some(cell) = row.cells_mut().first_mut() {
                *cell = Cell {
                    content: Content::Char(letter),
                    style: Style::PLAIN,
                };
            }
            screen.set_row(index, Arc::new(row)).unwrap();
        }
        screen
    }

    fn firsts(screen: &Screen) -> String {
        screen
            .rows()
            .iter()
            .filter_map(|row| row.text().chars().next())
            .collect()
    }

    #[test]
    fn sizes_outside_the_range_are_refused() {
        assert!(Size::new(0, 1).is_err());
        assert!(Size::new(1, MAX_CELLS + 1).is_err());
        assert!(Size::new(80, 24).is_ok());
    }

    #[test]
    fn scrolling_a_region_moves_only_its_rows() {
        let size = Size::new(2, 5).unwrap();
        let mut screen = lettered(size);
        screen.scroll_up(1, 3, 1, Style::PLAIN);
        assert_eq!(firsts(&screen), "acd e");
        screen.scroll_down(1, 3, 2, Style::PLAIN);
        assert_eq!(firsts(&screen), "a  ce");
    }

    #[test]
    fn scrolling_by_more_than_the_region_blanks_it() {
        let size = Size::new(1, 3).unwrap();
        let mut screen = lettered(size);
        screen.scroll_up(0, 2, 9, Style::PLAIN);
        assert_eq!(firsts(&screen), "   ");
    }

    #[test]
    fn a_row_of_the_wrong_width_is_refused() {
        let mut screen = Screen::blank(Size::new(3, 1).unwrap());
        let wide = Arc::new(Row::blank(4, Style::PLAIN));
        assert!(screen.set_row(0, wide).is_err());
    }

    #[test]
    fn resizing_keeps_the_top_left_and_the_cursor_inside() {
        let mut screen = lettered(Size::new(3, 4).unwrap());
        screen.cursor.col = 2;
        screen.cursor.row = 3;
        let small = screen.resized(Size::new(2, 2).unwrap());
        assert_eq!(small.text(), "a \nb ");
        assert_eq!((small.cursor.col, small.cursor.row), (1, 1));
    }
}
