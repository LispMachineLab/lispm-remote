// lispm-remote: how one screen becomes the next.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Updates: the difference between two screens, small enough to send.
//!
//! ```text
//!   inherit(base, size, i) = base.rows[i]      if i < base.rows ∧ base.cols = size.cols
//!                          = blank(size.cols)  otherwise
//!   apply(base, u).rows[i] = u.rows[i]                    if u replaces row i
//!                          = base.rows[from]              if u moves `from` to i
//!                          = inherit(base, u.size, i)     otherwise
//!
//!   Law (diff):  ∀ a b.  apply(a, diff(a, b)) = Ok(b)
//!   Law (split): ∀ a u.  fold(apply, a, split(u, n)) = apply(a, u)
//! ```
//!
//! [`diff`] sends a row only when no row of the base already holds it,
//! so scrolling costs a move per row, not the rows' cells.

use std::collections::HashMap;
use std::sync::Arc;

use crate::cell::Row;
use crate::screen::Cursor;
use crate::screen::Placement;
use crate::screen::Screen;
use crate::screen::Size;

/// Row `from` of the base, shown at row `to`.
///
/// Law: `Copy` is sound because a move is a pair of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Move {
    /// Where the row goes.
    pub to: u16,
    /// Where it was in the base.
    pub from: u16,
}

/// What turns a base screen into the next one.
///
/// Law: `Clone` duplicates a value; its rows are shared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The next screen's size.
    pub size: Size,
    /// Rows of the base shown elsewhere.
    pub moves: Vec<Move>,
    /// Rows that are new, by index.
    pub rows: Vec<(u16, Arc<Row>)>,
    /// The next cursor.
    pub cursor: Cursor,
    /// The next title.
    pub title: Option<String>,
    /// The next placements, or `None` when they are the base's.
    pub placements: Option<Vec<Placement>>,
}

/// Why an update does not apply to a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    /// A row index is past the bottom of the next screen.
    #[error("row {index} is past the screen's bottom")]
    RowOutOfRange {
        /// The index.
        index: u16,
    },
    /// A move takes a row the base does not have, or one of another width.
    #[error("row {from} cannot be moved")]
    MoveOutOfRange {
        /// The base's row.
        from: u16,
    },
    /// A new row is not as long as the next screen is wide.
    #[error("row {index} has the wrong length")]
    RowLength {
        /// The index.
        index: u16,
    },
}

/// Whether two rows show the same cells; shared rows are the same
/// without looking at their cells.
fn same_row(a: &Arc<Row>, b: &Arc<Row>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

/// Whether the base's rows can stand in the next screen's rows.
fn inheritable(base: &Screen, size: Size) -> bool {
    base.size().cols() == size.cols()
}

/// What row `index` of a screen of `size` is unless an update says
/// otherwise: the base's, or `blank`.
fn inherited<'a>(base: &'a Screen, size: Size, index: u16, blank: &'a Arc<Row>) -> &'a Arc<Row> {
    base.rows()
        .get(usize::from(index))
        .filter(|_| inheritable(base, size))
        .unwrap_or(blank)
}

fn blank_row(size: Size) -> Arc<Row> {
    Arc::new(Row::blank(size.cols(), crate::style::Style::PLAIN))
}

/// The update that turns `base` into `next`.
///
/// Post: `apply(base, &diff(base, next)) = Ok(next.clone())`.
#[must_use]
pub fn diff(base: &Screen, next: &Screen) -> Update {
    let size = next.size();
    let blank = blank_row(size);
    let inherit = inheritable(base, size);
    let mut found: HashMap<&Row, u16> = HashMap::new();
    if inherit {
        for (index, row) in (0..).zip(base.rows()) {
            found.entry(row.as_ref()).or_insert(index);
        }
    }
    let mut moves = Vec::new();
    let mut rows = Vec::new();
    for (index, row) in (0..).zip(next.rows()) {
        if same_row(inherited(base, size, index, &blank), row) {
            continue;
        }
        match found.get(row.as_ref()) {
            Some(&from) => moves.push(Move { to: index, from }),
            None => rows.push((index, Arc::clone(row))),
        }
    }
    Update {
        size,
        moves,
        rows,
        cursor: next.cursor,
        title: next.title.clone(),
        placements: (base.placements != next.placements).then(|| next.placements.clone()),
    }
}

/// The screen `update` makes of `base`.
///
/// # Errors
///
/// [`ApplyError`] when the update names rows that are not there; `base`
/// is then not the screen it was made for.
pub fn apply(base: &Screen, update: &Update) -> Result<Screen, ApplyError> {
    let size = update.size;
    let blank = blank_row(size);
    let mut rows: Vec<Arc<Row>> = (0..size.rows())
        .map(|index| Arc::clone(inherited(base, size, index, &blank)))
        .collect();
    for &Move { to, from } in &update.moves {
        let row = base
            .rows()
            .get(usize::from(from))
            .filter(|_| inheritable(base, size))
            .ok_or(ApplyError::MoveOutOfRange { from })?;
        let slot = rows
            .get_mut(usize::from(to))
            .ok_or(ApplyError::RowOutOfRange { index: to })?;
        *slot = Arc::clone(row);
    }
    for (index, row) in &update.rows {
        if !row.fits(size.cols()) {
            return Err(ApplyError::RowLength { index: *index });
        }
        let slot = rows
            .get_mut(usize::from(*index))
            .ok_or(ApplyError::RowOutOfRange { index: *index })?;
        *slot = Arc::clone(row);
    }
    let mut next = Screen::from_rows(size, rows).map_err(|_| ApplyError::RowLength { index: 0 })?;
    next.cursor = update.cursor;
    next.title.clone_from(&update.title);
    next.placements = update
        .placements
        .clone()
        .unwrap_or_else(|| base.placements.clone());
    Ok(next)
}

/// `update` as consecutive updates whose new rows hold at most `cells`
/// cells each (a single row more than that travels alone).  The first
/// carries the moves and placements; each applies to the screen the one
/// before made.
///
/// Post: `fold(apply, base, split(update, n)) = apply(base, update)`, and
/// the result is never empty.
#[must_use]
pub fn split(update: &Update, cells: usize) -> Vec<Update> {
    let width = usize::from(update.size.cols()).max(1);
    let per_part = (cells / width).max(1);
    let mut chunks = update.rows.chunks(per_part);
    let first = Update {
        rows: chunks.next().map(<[_]>::to_vec).unwrap_or_default(),
        ..update.clone()
    };
    let rest = chunks.map(|rows| Update {
        moves: Vec::new(),
        rows: rows.to_vec(),
        placements: None,
        ..update.clone()
    });
    std::iter::once(first).chain(rest).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::cell::Content;
    use crate::style::Style;
    use proptest::prelude::*;

    /// A screen whose row `i` shows `letters[i]` in its first cell.
    fn screen(cols: u16, letters: &[char]) -> Screen {
        let size = Size::new(cols, u16::try_from(letters.len()).unwrap()).unwrap();
        let mut screen = Screen::blank(size);
        for (index, &letter) in (0..).zip(letters) {
            let mut row = Row::blank(cols, Style::PLAIN);
            row.cells_mut()[0] = Cell {
                content: Content::Char(letter),
                style: Style::PLAIN,
            };
            screen.set_row(index, Arc::new(row)).unwrap();
        }
        screen
    }

    #[test]
    fn an_unchanged_screen_needs_no_row() {
        let a = screen(3, &['a', 'b']);
        let update = diff(&a, &a);
        assert!(update.rows.is_empty() && update.moves.is_empty());
        assert_eq!(apply(&a, &update), Ok(a));
    }

    #[test]
    fn scrolling_moves_rows_instead_of_sending_them() {
        let a = screen(4, &['a', 'b', 'c', 'd']);
        let b = screen(4, &['b', 'c', 'd', 'e']);
        let update = diff(&a, &b);
        assert_eq!(update.moves.len(), 3);
        assert_eq!(update.rows.len(), 1);
        assert_eq!(apply(&a, &update), Ok(b));
    }

    #[test]
    fn a_new_width_sends_every_row() {
        let a = screen(3, &['a', 'b']);
        let b = screen(5, &['a', 'b']);
        let update = diff(&a, &b);
        assert_eq!(update.rows.len(), 2);
        assert_eq!(apply(&a, &update), Ok(b));
    }

    #[test]
    fn an_update_for_another_base_is_refused() {
        let a = screen(3, &['a']);
        let b = screen(3, &['b', 'c']);
        let update = Update {
            moves: vec![Move { to: 0, from: 5 }],
            ..diff(&a, &b)
        };
        assert_eq!(
            apply(&a, &update),
            Err(ApplyError::MoveOutOfRange { from: 5 })
        );
        let below = Update {
            rows: vec![(7, Arc::new(Row::blank(3, Style::PLAIN)))],
            ..diff(&a, &b)
        };
        assert_eq!(
            apply(&a, &below),
            Err(ApplyError::RowOutOfRange { index: 7 })
        );
    }

    fn letters(rows: std::ops::Range<usize>) -> impl Strategy<Value = Vec<char>> {
        proptest::collection::vec(proptest::char::range('a', 'f'), rows)
    }

    proptest! {
        // Law: ∀ a b. apply(a, diff(a, b)) = Ok(b).
        #[test]
        fn applying_a_diff_gives_the_next_screen(
            a in letters(1..12), b in letters(1..12), wide in any::<bool>()
        ) {
            let a = screen(3, &a);
            let b = screen(if wide { 4 } else { 3 }, &b);
            prop_assert_eq!(apply(&a, &diff(&a, &b)), Ok(b));
        }

        // Law: ∀ a u n. fold(apply, a, split(u, n)) = apply(a, u).
        #[test]
        fn split_updates_apply_like_the_whole(
            a in letters(1..12), b in letters(1..12), cells in 1_usize..20, wide in any::<bool>()
        ) {
            let a = screen(3, &a);
            let b = screen(if wide { 4 } else { 3 }, &b);
            let update = diff(&a, &b);
            let mut screen = a.clone();
            for part in split(&update, cells) {
                screen = apply(&screen, &part).unwrap();
            }
            prop_assert_eq!(Ok(screen), apply(&a, &update));
        }
    }
}
