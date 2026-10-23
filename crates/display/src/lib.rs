// lispm-remote: LispM's windows, redrawn by a client.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The display protocol of LispM Remote.  A client (a page on a phone or
//! a computer) shows LispM's windows by redrawing them: LispM says what
//! each cell holds and where each image goes, and the client draws that
//! with its own fonts and from its own pool of images.  No pixels travel.
//!
//! # Model
//!
//! ```text
//!   Screen    = (size, rows : [Row; size.rows], cursor, title, placements)
//!   Row       = [Cell; size.cols]
//!   Cell      = (content, style)          content = char | cluster | spacer
//!   Placement = (key, resource, cells, crop, offset, z)
//!   resource  = SHA-256(bytes)            never inside a frame
//! ```
//!
//! LispM keeps one screen per surface (one LispM window).  A client keeps
//! a copy at some sequence number `s` and receives
//! `Frame { base: s, seq: s', update }` with
//!
//! ```text
//!   apply(screen_s, update) = Ok(screen_s')        (see `update`)
//!   apply(a, diff(a, b))     = Ok(b)               for all screens a, b
//! ```
//!
//! A client whose copy is not at `base` asks for a fresh copy (`Resync`),
//! so lost or late frames cost a redraw, never a wrong screen.
//!
//! A placement names its image by content hash.  The client draws it from
//! its pool and sends `Want` only for an image it has never seen; the
//! answer arrives as `Resource` chunks, which the client checks against
//! the hash before keeping them.
//!
//! # On a LispM Remote channel
//!
//! A channel's payloads are told apart by their first byte: `0x01` starts
//! a message of this protocol (it is the codec's version byte), `{` a JSON
//! control message, and every other first byte is reserved.  A message is
//! at most [`MAX_MESSAGE_LEN`] bytes, which one record carries.  Records
//! may be lost or arrive out of order.
//!
//! # Shape
//!
//! Everything here is a value or a pure function: the screen model, the
//! diff, and the codec of both directions' messages.  Terminals, sockets
//! and canvases live in the crates around this one.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod cell;
mod codec;
mod color;
mod input;
mod message;
mod resource;
mod screen;
mod style;
mod update;

pub use cell::Cell;
pub use cell::Content;
pub use cell::Row;
pub use codec::CodecError;
pub use codec::MAX_MESSAGE_LEN;
pub use color::Color;
pub use color::Rgb;
pub use color::xterm_color;
pub use input::Button;
pub use input::Input;
pub use input::Key;
pub use input::KeyCode;
pub use input::Mods;
pub use input::Named;
pub use input::Pointer;
pub use input::PointerKind;
pub use message::CellPixels;
pub use message::ClientMessage;
pub use message::Seq;
pub use message::ServerMessage;
pub use message::SurfaceId;
pub use message::Window;
pub use resource::RESOURCE_CHUNK;
pub use resource::ResourceId;
pub use screen::CellRect;
pub use screen::Cursor;
pub use screen::CursorShape;
pub use screen::MAX_CELLS;
pub use screen::PixelRect;
pub use screen::Placement;
pub use screen::PlacementKey;
pub use screen::Screen;
pub use screen::Size;
pub use screen::SizeError;
pub use style::Attrs;
pub use style::Style;
pub use style::Underline;
pub use update::ApplyError;
pub use update::Move;
pub use update::Update;
pub use update::apply;
pub use update::diff;
pub use update::split;
