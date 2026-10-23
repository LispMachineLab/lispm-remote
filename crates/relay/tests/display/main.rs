// lispm-remote: integration tests of the relay, over real sockets.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The relay's server with a client on a real WebSocket and a surface on
//! a real pseudo-terminal (`server`).
//!
//! The modules are `cfg(test)` items, so clippy's test allowances reach
//! their helpers.

#[cfg(test)]
mod server;
