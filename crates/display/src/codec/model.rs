// lispm-remote: screens and updates as bytes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The byte forms of the screen model.  A row travels as runs of cells
//! sharing a style, so a line of plain text costs one style.

use std::sync::Arc;

use super::CodecError;
use super::Reader;
use super::Wire;
use super::wire_struct;
use super::wire_tags;
use crate::cell::Cell;
use crate::cell::Content;
use crate::cell::Row;
use crate::color::Color;
use crate::color::Rgb;
use crate::resource::ResourceId;
use crate::screen::CellRect;
use crate::screen::Cursor;
use crate::screen::CursorShape;
use crate::screen::PixelRect;
use crate::screen::Placement;
use crate::screen::PlacementKey;
use crate::screen::Size;
use crate::style::Attrs;
use crate::style::Style;
use crate::style::Underline;
use crate::update::Move;
use crate::update::Update;

impl Wire for Size {
    fn put(&self, out: &mut Vec<u8>) {
        self.cols().put(out);
        self.rows().put(out);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let cols = u16::get(input)?;
        let rows = u16::get(input)?;
        Self::new(cols, rows).map_err(CodecError::Size)
    }
}

wire_struct!(Rgb { r, g, b });

impl Wire for Color {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Default => out.push(0),
            Self::Rgb(rgb) => {
                out.push(1);
                rgb.put(out);
            }
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(Self::Default),
            1 => Ok(Self::Rgb(Rgb::get(input)?)),
            tag => Err(CodecError::Tag {
                what: "colour",
                tag,
            }),
        }
    }
}

impl Wire for Attrs {
    fn put(&self, out: &mut Vec<u8>) {
        self.bits().put(out);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self::from_bits(u16::get(input)?))
    }
}

wire_tags!(Underline ("underline") {
    None = 0,
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
});

wire_struct!(Style {
    fg,
    bg,
    attrs,
    underline,
    underline_color
});

impl Wire for Content {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Char(c) => {
                out.push(0);
                c.put(out);
            }
            Self::Cluster(text) => {
                out.push(1);
                text.to_string().put(out);
            }
            Self::Spacer => out.push(2),
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(Self::Char(char::get(input)?)),
            1 => Ok(Self::Cluster(Arc::from(String::get(input)?))),
            2 => Ok(Self::Spacer),
            tag => Err(CodecError::Tag {
                what: "content",
                tag,
            }),
        }
    }
}

/// A row as `cells ‖ run*`, where a run of one style is
/// `style ‖ pieces ‖ piece*` and a piece is one of
///
/// ```text
///   0 ‖ string    characters, one cell each
///   1 ‖ string    wide characters, each a cell and its spacer
///   2 ‖ content   any one cell
/// ```
///
/// so a line of text costs its UTF-8 and a style.
impl Wire for Row {
    fn put(&self, out: &mut Vec<u8>) {
        let cells = self.cells();
        put_len(out, cells.len());
        for run in cells.chunk_by(|a, b| a.style == b.style) {
            if let Some(first) = run.first() {
                first.style.put(out);
                let pieces = pieces(run);
                put_len(out, pieces.len());
                for piece in pieces {
                    piece.put(out);
                }
            }
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let total = input.count()?;
        let mut cells = Vec::with_capacity(total);
        while cells.len() < total {
            let style = Style::get(input)?;
            let pieces = input.count()?;
            if pieces == 0 {
                return Err(CodecError::Runs);
            }
            for _ in 0..pieces {
                Piece::get(input)?.cells(style, &mut cells);
                if cells.len() > total {
                    return Err(CodecError::Runs);
                }
            }
        }
        Ok(Self::new(cells))
    }
}

/// A piece of a run: cells that share a cheap byte form.
#[derive(Debug, PartialEq, Eq)]
enum Piece {
    /// Characters, one cell each.
    Narrow(String),
    /// Wide characters, each followed by its spacer.
    Wide(String),
    /// One cell of any content.
    One(Content),
}

impl Piece {
    /// Appends the cells this piece stands for, in `style`.
    fn cells(self, style: Style, cells: &mut Vec<Cell>) {
        let mut push = |content| cells.push(Cell { content, style });
        match self {
            Self::Narrow(text) => text.chars().for_each(|c| push(Content::Char(c))),
            Self::Wide(text) => text.chars().for_each(|c| {
                push(Content::Char(c));
                push(Content::Spacer);
            }),
            Self::One(content) => push(content),
        }
    }
}

impl Wire for Piece {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Narrow(text) => {
                out.push(0);
                text.put(out);
            }
            Self::Wide(text) => {
                out.push(1);
                text.put(out);
            }
            Self::One(content) => {
                out.push(2);
                content.put(out);
            }
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(Self::Narrow(String::get(input)?)),
            1 => Ok(Self::Wide(String::get(input)?)),
            2 => Ok(Self::One(Content::get(input)?)),
            tag => Err(CodecError::Tag { what: "piece", tag }),
        }
    }
}

/// The pieces of a run, greedily: a character followed by a spacer
/// joins a wide piece, a character not followed by one a narrow piece,
/// anything else stands alone.
///
/// Law: the pieces' cells, in `style`, are `run`.
fn pieces(run: &[Cell]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut rest = run;
    while let Some((first, after)) = rest.split_first() {
        let wide = |cells: &[Cell]| {
            matches!(
                cells,
                [
                    Cell {
                        content: Content::Char(_),
                        ..
                    },
                    Cell {
                        content: Content::Spacer,
                        ..
                    },
                    ..
                ]
            )
        };
        let narrow = |cells: &[Cell]| {
            matches!(
                cells,
                [
                    Cell {
                        content: Content::Char(_),
                        ..
                    },
                    ..
                ]
            ) && !wide(cells)
        };
        if wide(rest) {
            let mut text = String::new();
            while wide(rest) {
                if let [
                    Cell {
                        content: Content::Char(c),
                        ..
                    },
                    _,
                    tail @ ..,
                ] = rest
                {
                    text.push(*c);
                    rest = tail;
                }
            }
            pieces.push(Piece::Wide(text));
        } else if narrow(rest) {
            let mut text = String::new();
            while narrow(rest) {
                if let [
                    Cell {
                        content: Content::Char(c),
                        ..
                    },
                    tail @ ..,
                ] = rest
                {
                    text.push(*c);
                    rest = tail;
                }
            }
            pieces.push(Piece::Narrow(text));
        } else {
            pieces.push(Piece::One(first.content.clone()));
            rest = after;
        }
    }
    pieces
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    super::put_varint(out, u64::try_from(len).unwrap_or(u64::MAX));
}

impl Wire for Arc<Row> {
    fn put(&self, out: &mut Vec<u8>) {
        self.as_ref().put(out);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self::new(Row::get(input)?))
    }
}

impl<A: Wire, B: Wire> Wire for (A, B) {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
        self.1.put(out);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok((A::get(input)?, B::get(input)?))
    }
}

wire_tags!(CursorShape ("cursor shape") {
    Block = 0,
    Underline = 1,
    Bar = 2,
});

wire_struct!(Cursor {
    col,
    row,
    visible,
    shape
});
wire_struct!(CellRect {
    col,
    row,
    cols,
    rows
});
wire_struct!(PixelRect { x, y, w, h });
wire_struct!(PlacementKey { image, placement });

impl Wire for ResourceId {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self.hash());
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let bytes = input.take(32)?;
        let hash: [u8; 32] = bytes.try_into().map_err(|_| CodecError::Truncated)?;
        Ok(Self::from_hash(hash))
    }
}

wire_struct!(Placement {
    key,
    resource,
    cells,
    crop,
    offset_x,
    offset_y,
    z
});
wire_struct!(Move { to, from });
wire_struct!(Update {
    size,
    moves,
    rows,
    cursor,
    title,
    placements
});
