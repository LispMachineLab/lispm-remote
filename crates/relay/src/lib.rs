// lispm-remote: Emacs tty frames as display surfaces.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The relay between Emacs and the display's clients.  Emacs does all
//! the layout: it draws a LispM window on a tty frame whose terminal is
//! a pseudo-terminal this crate holds, as it would for `emacsclient -t`.
//! The relay reads what Emacs writes as an xterm would, keeps the screen
//! it describes, and sends clients that screen as the display protocol
//! (`lispm-remote-display`): cells and styles, and images as references
//! to the clients' pools.  What clients do comes back as the input an
//! xterm would send.  Nothing in Emacs changes.
//!
//! ```text
//!   Emacs ──tty──▶ Pty ──▶ Terminal ──Screen──▶ Follower ──Frame──▶ client
//!   Emacs ◀─tty─── Pty ◀── keys::encode ◀──────Input─────────────── client
//! ```
//!
//! Pure parts: [`apc`], [`kitty`], [`terminal`], [`keys`], [`follow`],
//! [`order`].
//! Effects: [`pty`] (the system's terminals), [`surface`] (a reader
//! thread per surface), [`hub`] (what Emacs and sessions share),
//! [`session`], and its transports: [`server`] (WebSockets) and
//! [`channel`] (LispM Remote's channels, carried by Emacs).

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod apc;
pub mod channel;
pub mod follow;
pub mod hub;
pub mod keys;
pub mod kitty;
pub mod order;
pub mod pool;
pub mod pty;
pub mod server;
pub mod session;
pub mod surface;
pub mod terminal;

pub use hub::Hub;
pub use hub::Request;
pub use hub::Wake;
pub use server::Server;
pub use surface::Surface;
