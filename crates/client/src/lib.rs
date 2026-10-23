// lispm-remote: the display's client.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The display's client, compiled to WebAssembly for a page in any
//! browser: LispM's windows redrawn on a canvas, from the cells and
//! image references LispM sends, with the client's own fonts and pool.
//!
//! [`model`] (what the client knows and how messages change it) and
//! [`keymap`] (what a browser's keys mean) are pure and tested natively;
//! the page itself (`app`, `render`) exists only in WebAssembly.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod keymap;
pub mod model;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod render;
