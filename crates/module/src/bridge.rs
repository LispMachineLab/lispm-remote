// lispm-remote: LispM Remote's namespace on a Rings node.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Rings protocol registered under [`NAMESPACE`].  It keeps no state
//! and decides nothing: every frame a peer sends is handed to Emacs, where
//! the pairing and session machines judge it.
//!
//! ```text
//!   decode(w) = Inbound(w.from, w.payload)   if w.from ≠ w.me, else Reject
//!   step((), i) = ((), [i])
//!   run(i)      = inbox.push(Frame(i.from, i.payload))
//! ```

use std::sync::Arc;

use bytes::Bytes;
use rings_core::dht::Did;
use rings_node::extension::ext::Ctx;
use rings_node::extension::ext::EffectScope;
use rings_node::extension::ext::Interpret;
use rings_node::extension::ext::Protocol;
use rings_node::extension::ext::Reject;
use rings_node::extension::ext::Transition;
use rings_node::extension::ext::Wire;

use crate::node::Event;
use crate::node::Inbox;
use crate::node::NAMESPACE;

/// A frame from a peer, as Rings authenticated it.
pub(crate) struct Inbound {
    from: Did,
    payload: Vec<u8>,
}

/// The protocol: forward every peer's frame.
pub(crate) struct Forward;

impl Protocol for Forward {
    type State = ();
    type Event = Inbound;
    type Effect = Inbound;

    fn namespace(&self) -> &str {
        NAMESPACE
    }

    fn init(&self) {}

    fn decode(&self, wire: Wire<'_>) -> Result<Inbound, Reject> {
        if wire.from == wire.me {
            // Nothing in this namespace is injected locally.
            Err(Reject("self-injected".to_owned()))
        } else {
            Ok(Inbound {
                from: wire.from,
                payload: wire.payload.to_vec(),
            })
        }
    }

    fn step(&self, _ctx: Ctx<'_, ()>, event: Inbound) -> Transition<(), Inbound> {
        Transition::with((), vec![event])
    }
}

/// The interpreter: put the frame in Emacs's inbox.
pub(crate) struct Shell {
    inbox: Arc<Inbox>,
}

impl Shell {
    pub(crate) const fn new(inbox: Arc<Inbox>) -> Self {
        Self { inbox }
    }
}

#[async_trait::async_trait]
impl Interpret for Shell {
    type Effect = Inbound;

    async fn run(
        &self,
        _scope: &EffectScope,
        effect: Inbound,
    ) -> rings_node::error::Result<Vec<Bytes>> {
        self.inbox.push(Event::Frame {
            from: effect.from.to_string(),
            payload: effect.payload,
        });
        Ok(Vec::new())
    }
}
