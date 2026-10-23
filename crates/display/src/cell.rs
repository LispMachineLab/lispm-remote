// lispm-remote: cells and rows.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A cell holds one character, a cluster of them, or the right half of a
//! wide character to its left.

use std::sync::Arc;

use crate::style::Style;

/// What a cell shows.
///
/// Law: `Clone` shares a cluster's text, which is immutable, so a clone
/// is indistinguishable from the original.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Content {
    /// One character; a blank cell is `Char(' ')`.
    Char(char),
    /// A character with the combining characters drawn over it.
    Cluster(Arc<str>),
    /// The right half of the wide character in the cell to the left.
    Spacer,
}

impl Content {
    /// The blank: a space.
    pub const BLANK: Self = Self::Char(' ');

    /// Whether this is the right half of a wide character.
    #[must_use]
    pub const fn is_spacer(&self) -> bool {
        matches!(self, Self::Spacer)
    }

    /// The text the cell shows, empty for a spacer.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Char(c) => c.to_string(),
            Self::Cluster(text) => text.to_string(),
            Self::Spacer => String::new(),
        }
    }
}

/// One cell of a row.
///
/// Law: `Clone` duplicates a value; see [`Content`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    /// What it shows.
    pub content: Content,
    /// How it looks.
    pub style: Style,
}

impl Cell {
    /// A blank cell in `style`.
    #[must_use]
    pub const fn blank(style: Style) -> Self {
        Self {
            content: Content::BLANK,
            style,
        }
    }
}

/// A row of cells.  Its length is the screen's width; [`crate::Screen`]
/// and [`crate::apply`] keep that true.
///
/// Law: `Clone` duplicates a value; rows are shared between screens
/// through `Arc`, and changed by copy on write.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Row(Vec<Cell>);

impl Row {
    /// A row of `cols` blank cells in `style`.
    #[must_use]
    pub fn blank(cols: u16, style: Style) -> Self {
        Self(vec![Cell::blank(style); usize::from(cols)])
    }

    /// The row holding `cells`.
    #[must_use]
    pub const fn new(cells: Vec<Cell>) -> Self {
        Self(cells)
    }

    /// The cells, left to right.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.0
    }

    /// The cells, to change in place.  A slice, so the row keeps its
    /// length.
    pub fn cells_mut(&mut self) -> &mut [Cell] {
        &mut self.0
    }

    /// The number of cells.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the row has no cell.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the row is `cols` cells long.
    #[must_use]
    pub fn fits(&self, cols: u16) -> bool {
        self.0.len() == usize::from(cols)
    }

    /// The row's text: every cell's, spacers left out.
    #[must_use]
    pub fn text(&self) -> String {
        self.0.iter().map(|cell| cell.content.text()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_row_is_spaces_in_its_style() {
        let row = Row::blank(3, Style::PLAIN);
        assert_eq!(row.text(), "   ");
        assert!(row.fits(3));
        assert!(!row.fits(4));
    }

    #[test]
    fn a_spacer_adds_no_text() {
        let row = Row::new(vec![
            Cell {
                content: Content::Char('\u{4f60}'),
                style: Style::PLAIN,
            },
            Cell {
                content: Content::Spacer,
                style: Style::PLAIN,
            },
        ]);
        assert_eq!(row.text(), "\u{4f60}");
    }
}
