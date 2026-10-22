// lispm-remote: each record at most once, in any order.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The transport may lose, duplicate and reorder frames, so records carry
//! their nonce and the receiver keeps a sliding window over the nonces it
//! has accepted:
//!
//! ```text
//!   W        = 128
//!   S        = (top : Option ℕ, seen : 𝔹^W)    seen[i] ⇔ top − i accepted
//!   accepts(n) ⇔ top = None ∨ n > top ∨ (top − n < W ∧ ¬seen[top − n])
//!   mark(n)    = top ← max(top, n); shift seen by the rise; seen[top − n] ← 1
//! ```
//!
//! It refines the unbounded model `M = (accepted : ℘ ℕ, top)` with
//! `accepts_M(n) ⇔ n ∉ accepted ∧ (top = None ∨ n + W > top)`: the window
//! forgets only nonces too old to be accepted anyway.  Hence each nonce is
//! accepted at most once, and a record delayed by fewer than `W` later
//! ones still arrives.

use crate::wire::Nonce;

/// The window's width in nonces.
pub const WINDOW: u64 = 128;

/// The nonces a channel's receiving side has accepted, as far back as
/// matters.
///
/// Law: `Copy` is sound because the window is a value the channel owns;
/// copies are compared in tests and never shared between channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayWindow {
    top: Option<u64>,
    seen: u128,
}

impl ReplayWindow {
    /// Whether a record sealed under `nonce` may be accepted.
    #[must_use]
    pub fn accepts(&self, nonce: Nonce) -> bool {
        let n = nonce.get();
        match self.top {
            None => true,
            Some(top) if n > top => true,
            Some(top) => bit(top - n).is_some_and(|mask| self.seen & mask == 0),
        }
    }

    /// The window after accepting `nonce`.
    ///
    /// Pre: `self.accepts(nonce)`.  Post: `¬result.accepts(nonce)`.
    #[must_use]
    pub fn mark(self, nonce: Nonce) -> Self {
        let n = nonce.get();
        match self.top {
            None => Self {
                top: Some(n),
                seen: 1,
            },
            Some(top) if n > top => Self {
                top: Some(n),
                seen: shifted(self.seen, n - top) | 1,
            },
            Some(top) => Self {
                top: Some(top),
                seen: self.seen | bit(top - n).unwrap_or(0),
            },
        }
    }
}

/// The mask of offset `offset` back from the top, if inside the window.
fn bit(offset: u64) -> Option<u128> {
    u32::try_from(offset)
        .ok()
        .and_then(|offset| 1_u128.checked_shl(offset))
}

/// `seen` moved back by `rise` places; what falls out of the window is lost.
fn shifted(seen: u128, rise: u64) -> u128 {
    u32::try_from(rise)
        .ok()
        .and_then(|rise| seen.checked_shl(rise))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    /// The unbounded model the window refines.
    #[derive(Default)]
    struct Model {
        accepted: BTreeSet<u64>,
        top: Option<u64>,
    }

    impl Model {
        fn accepts(&self, n: u64) -> bool {
            !self.accepted.contains(&n) && self.top.is_none_or(|top| n.saturating_add(WINDOW) > top)
        }

        fn mark(&mut self, n: u64) {
            self.accepted.insert(n);
            self.top = Some(self.top.map_or(n, |top| top.max(n)));
        }
    }

    fn nonces() -> impl Strategy<Value = Vec<u64>> {
        // Values near each other, so the window's edge is exercised.
        (any::<u64>(), proptest::collection::vec(0_u64..400, 0..300)).prop_map(|(base, offsets)| {
            let base = base.min(u64::MAX - 400);
            offsets.into_iter().map(|o| base + o).collect()
        })
    }

    proptest! {
        // Refinement: at every step, the window accepts exactly what the
        // unbounded model accepts.
        #[test]
        fn the_window_refines_the_unbounded_model(sequence in nonces()) {
            let mut window = ReplayWindow::default();
            let mut model = Model::default();
            for n in sequence {
                let accepted = window.accepts(Nonce::new(n));
                prop_assert_eq!(accepted, model.accepts(n));
                if accepted {
                    window = window.mark(Nonce::new(n));
                    model.mark(n);
                }
            }
        }

        // Law: after mark(n), n is never accepted again.
        #[test]
        fn a_marked_nonce_is_not_accepted_again(sequence in nonces()) {
            let mut window = ReplayWindow::default();
            for n in sequence {
                if window.accepts(Nonce::new(n)) {
                    window = window.mark(Nonce::new(n));
                }
                prop_assert!(!window.accepts(Nonce::new(n)));
            }
        }
    }

    #[test]
    fn records_delayed_within_the_window_still_arrive() {
        let window = ReplayWindow::default().mark(Nonce::new(WINDOW - 1));
        assert!(window.accepts(Nonce::new(0)));
        let window = window.mark(Nonce::new(WINDOW));
        assert!(!window.accepts(Nonce::new(0)));
    }

    #[test]
    fn the_last_nonce_is_accepted() {
        let window = ReplayWindow::default().mark(Nonce::new(0));
        assert!(window.accepts(Nonce::new(u64::MAX)));
        let window = window.mark(Nonce::new(u64::MAX));
        assert!(!window.accepts(Nonce::new(u64::MAX)));
        assert!(!window.accepts(Nonce::new(0)));
    }
}
