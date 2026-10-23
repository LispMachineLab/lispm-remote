// lispm-remote: the messages of the display protocol.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What LispM tells a client, and what a client tells LispM.
//!
//! ```text
//!   client                                  LispM
//!     → Hello n                               start showing LispM's windows;
//!                                             input is numbered from n
//!     ← Windows [w…]                          what can be opened
//!     → Open w size cell
//!     ← Opened s w size
//!     ← Frame s base=0 seq=1 update           a whole screen
//!     → Ack s 1
//!     ← Frame s base=1 seq=2 update           a difference
//!     → Input n [(s, i)…]                     keys, text, pointer; all unacknowledged
//!     ← InputAck n                            taken in order up to n
//!     → Want r                                an image it lacks
//!     ← Resource r total offset bytes …       its bytes, in chunks
//!     → Resync s                              its copy is lost: base=0 again
//!     → Close s  /  ← Closed s
//!     → Bye                                   stop: send nothing more
//! ```
//!
//! Over a WebSocket the connection says hello and goodbye itself; over a
//! LispM Remote channel, which also carries other messages, `Hello` and
//! `Bye` start and end the display's session.

use crate::input::Input;
use crate::resource::ResourceId;
use crate::screen::Size;
use crate::update::Update;

/// A surface: one LispM window shown on clients.
///
/// Law: `Copy` is sound because an id is a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId(u32);

impl SurfaceId {
    /// The surface numbered `n`.
    #[must_use]
    pub const fn new(n: u32) -> Self {
        Self(n)
    }

    /// Its number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// The number of a surface's screen state; `Seq::ZERO` is "nothing",
/// the base of a whole screen.
///
/// Law: `Copy` is sound because a sequence number is a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(u64);

impl Seq {
    /// No state: frames based on it carry whole screens.
    pub const ZERO: Self = Self(0);

    /// The state numbered `n`.
    #[must_use]
    pub const fn new(n: u64) -> Self {
        Self(n)
    }

    /// Its number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The state after this one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// A LispM window a client may open.
///
/// Law: `Clone` duplicates a value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Window {
    /// What names it to LispM.
    pub name: String,
    /// What a person reads.
    pub title: String,
}

/// The size of one cell on the client, in device pixels.
///
/// Law: `Copy` is sound because a size is a pair of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellPixels {
    /// The width.
    pub width: u16,
    /// The height.
    pub height: u16,
}

/// What LispM tells a client.
///
/// Law: `Clone` duplicates a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerMessage {
    /// The windows that can be opened now.
    Windows(Vec<Window>),
    /// A surface showing `window` exists, at `size`.
    Opened {
        /// The surface.
        surface: SurfaceId,
        /// The window it shows.
        window: String,
        /// Its size.
        size: Size,
    },
    /// The surface's screen moved from state `base` to state `seq`.
    Frame {
        /// The surface.
        surface: SurfaceId,
        /// The state `update` applies to.
        base: Seq,
        /// The state it makes.
        seq: Seq,
        /// Whether more frames of the same change follow: a client draws
        /// once one comes without.
        more: bool,
        /// The change.
        update: Update,
    },
    /// The surface is gone.
    Closed {
        /// The surface.
        surface: SurfaceId,
    },
    /// Bytes `offset..offset + bytes.len()` of the resource `id`, whose
    /// length is `total`.
    Resource {
        /// The resource.
        id: ResourceId,
        /// Its length.
        total: u32,
        /// Where these bytes start.
        offset: u32,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// LispM does not have the resource `id`.
    Unknown {
        /// The resource.
        id: ResourceId,
    },
    /// Text LispM put on the clipboard.
    Clipboard(String),
    /// The surface rang its bell.
    Bell {
        /// The surface.
        surface: SurfaceId,
    },
    /// LispM took the client's input up to `seq`, in order: the client
    /// stops resending it.
    InputAck {
        /// The last input taken.
        seq: Seq,
    },
}

/// What a client tells LispM.
///
/// Law: `Clone` duplicates a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMessage {
    /// Open `window` on a surface of `size`.
    Open {
        /// The window, by name.
        window: String,
        /// The surface's size.
        size: Size,
        /// A cell's size.
        cell: CellPixels,
    },
    /// Close the surface.
    Close {
        /// The surface.
        surface: SurfaceId,
    },
    /// The surface has a new size on this client.
    Resize {
        /// The surface.
        surface: SurfaceId,
        /// Its size.
        size: Size,
        /// A cell's size.
        cell: CellPixels,
    },
    /// What a person did, each on a surface.  Inputs are numbered on
    /// from the number the session's `Hello` gave (1 without one),
    /// `inputs[i]` being number `first + i`, and a client sends all it
    /// has not seen acknowledged in each message, so LispM takes them
    /// once and in order over a channel that loses and reorders.
    Input {
        /// The number of the first input.
        first: Seq,
        /// The inputs, and the surface each was made on.
        inputs: Vec<(SurfaceId, Input)>,
    },
    /// The client's copy of the surface is at `seq`.
    Ack {
        /// The surface.
        surface: SurfaceId,
        /// The state it applied last.
        seq: Seq,
    },
    /// The client has no usable copy of the surface: send it whole.
    Resync {
        /// The surface.
        surface: SurfaceId,
    },
    /// The client lacks the resource `id`.
    Want {
        /// The resource.
        id: ResourceId,
    },
    /// The client starts showing LispM's windows.  Its input in this
    /// session is numbered from `first`, past any it sent before, so
    /// input of an earlier session that arrives late is never taken.
    Hello {
        /// The number of its next input.
        first: Seq,
    },
    /// The client stops showing them: send it nothing more.
    Bye,
}
