// lispm-remote: integration tests on real Rings nodes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real Rings nodes in this process, connected over WebRTC on this
//! computer: frames between two of them (`two_nodes`); LispM Remote's
//! pairing and sessions carried by the network, relayed by a third node
//! (`remote`); and the same over the public network, when asked
//! (`public`).
//!
//! The modules are `cfg(test)` items, so clippy's test allowances reach
//! their helpers.

#[cfg(test)]
mod public;
#[cfg(test)]
mod remote;
#[cfg(test)]
mod support;
#[cfg(test)]
mod two_nodes;
