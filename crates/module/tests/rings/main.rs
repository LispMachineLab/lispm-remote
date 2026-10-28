// lispm-remote: integration tests on real Rings nodes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real Rings nodes in this process, connected over WebRTC on this
//! computer: frames between two of them (`two_nodes`); LispM Remote's
//! pairing and sessions carried by the network, relayed by a third node
//! (`remote`); the same over the public network, when asked (`public`);
//! a node that renews its delegation and talks on (`renewal`); and what a
//! node of another Rings version says (`versions`).
//!
//! The modules are `cfg(test)` items, so clippy's test allowances reach
//! their helpers.

#[cfg(test)]
mod public;
#[cfg(test)]
mod remote;
#[cfg(test)]
mod renewal;
#[cfg(test)]
mod support;
#[cfg(test)]
mod two_nodes;
#[cfg(test)]
mod versions;
