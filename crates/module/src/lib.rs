// lispm-remote: a Rings Network node inside Emacs.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Emacs module of LispM Remote: a Rings Network node running on its
//! own threads inside Emacs, so LispM is reachable by DID wherever it runs,
//! behind any NAT, while Emacs is up.
//!
//! The node registers one Rings protocol, under [`NAMESPACE`], that hands
//! every peer's frame to Emacs (`bridge`); the pairing and session
//! machines of `lispm-remote-protocol` judge the frames there.  [`node`]
//! is the Rust side, usable and tested without Emacs; the Lisp functions
//! (`lisp`) wrap it one to one.
//!
//! Rings's account is a secp256k1 key the shell stores; the node runs on a
//! fresh session key that account delegates to at each start.

#![deny(missing_docs)]

mod bridge;
mod lisp;
pub mod node;

pub use node::Account;
pub use node::Event;
pub use node::MAX_QUEUED;
pub use node::NAMESPACE;
pub use node::Node;
pub use node::NodeError;
pub use node::Wake;

mod abi {
    //! What Emacs asks of a module before loading it.  The macros below
    //! define the symbols Emacs looks up; they generate public items whose
    //! docs cannot be written here.
    #![allow(missing_docs)]

    emacs::plugin_is_GPL_compatible!();

    #[emacs::module(
        name = "lispm-remote-core",
        defun_prefix = "lispm-remote-core",
        separator = "-",
        mod_in_name = false
    )]
    fn init(_: &emacs::Env) -> emacs::Result<()> {
        Ok(())
    }
}
