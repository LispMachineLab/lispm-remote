// lispm-remote: a client's copy of a surface, over a channel that loses.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A replica: the client's side of a surface's synchronisation, for a
//! channel that may lose, repeat and reorder frames.
//!
//! LispM bases each frame on the newest state the client acknowledged,
//! and acknowledgements may themselves be lost, so a frame may be based
//! on a state older than the client's newest.  The replica therefore
//! keeps the recent states it applied, by sequence number:
//!
//! ```text
//!   Replica  = (states : Seq ⇀ Screen, latest : Seq)
//!   receive(base, seq, u):
//!     seq ≤ latest                 → Stale            (old or repeated)
//!     base = 0                     → apply to a blank screen
//!     base ∈ dom states            → apply to states[base]
//!     otherwise                    → Unbased          (ask for the whole screen)
//!     on apply: states[seq] ← next ; latest ← seq ; forget states before base
//! ```
//!
//! Forgetting is safe: LispM's acknowledged state only grows, so a later
//! frame is never based on a state older than an earlier frame's base;
//! a frame that was sent earlier but arrives later has a smaller `seq`
//! and is stale.

use std::collections::BTreeMap;

use crate::message::Seq;
use crate::screen::Screen;
use crate::update::Update;
use crate::update::apply;

/// The most states a replica keeps.
pub const MAX_KEPT: usize = 32;

/// What became of a frame.
///
/// Law: `Copy` is sound because an outcome is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// It applied: the replica is now at `seq`, which the client should
    /// acknowledge.
    Applied {
        /// The state it made.
        seq: Seq,
    },
    /// It is older than what the replica has, or a repeat.
    Stale,
    /// The replica lacks the state it is based on: ask for a whole
    /// screen (`Resync`).
    Unbased,
}

/// A client's copy of one surface.
#[derive(Clone, Debug, Default)]
pub struct Replica {
    states: BTreeMap<Seq, Screen>,
    latest: Seq,
}

impl Replica {
    /// A replica with nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The newest screen, if any frame applied yet.
    #[must_use]
    pub fn screen(&self) -> Option<&Screen> {
        self.states.get(&self.latest)
    }

    /// The newest state's number.
    #[must_use]
    pub const fn latest(&self) -> Seq {
        self.latest
    }

    /// Forgets every state, as after asking for a whole screen.
    pub fn reset(&mut self) {
        self.states.clear();
    }

    /// Takes in the frame that makes state `seq` from state `base`.
    pub fn receive(&mut self, base: Seq, seq: Seq, update: &Update) -> Received {
        if self.screen().is_some() && seq <= self.latest {
            return Received::Stale;
        }
        let from = if base == Seq::ZERO {
            Screen::blank(update.size)
        } else {
            match self.states.get(&base) {
                Some(screen) => screen.clone(),
                None => return Received::Unbased,
            }
        };
        let Ok(next) = apply(&from, update) else {
            return Received::Unbased;
        };
        self.states.retain(|kept, _| *kept >= base);
        self.states.insert(seq, next);
        self.latest = seq;
        while self.states.len() > MAX_KEPT {
            self.states.pop_first();
        }
        Received::Applied { seq }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::cell::Cell;
    use crate::cell::Content;
    use crate::cell::Row;
    use crate::screen::Size;
    use crate::style::Style;
    use crate::update::diff;

    fn screen(letter: char) -> Screen {
        let size = Size::new(2, 1).unwrap();
        let mut screen = Screen::blank(size);
        let row = Row::new(vec![
            Cell {
                content: Content::Char(letter),
                style: Style::PLAIN
            };
            2
        ]);
        screen.set_row(0, Arc::new(row)).unwrap();
        screen
    }

    fn blank() -> Screen {
        Screen::blank(Size::new(2, 1).unwrap())
    }

    #[test]
    fn frames_based_on_an_older_kept_state_apply() {
        let mut replica = Replica::new();
        let a = screen('a');
        assert_eq!(
            replica.receive(Seq::ZERO, Seq::new(1), &diff(&blank(), &a)),
            Received::Applied { seq: Seq::new(1) }
        );
        assert_eq!(
            replica.receive(Seq::new(1), Seq::new(2), &diff(&a, &screen('b'))),
            Received::Applied { seq: Seq::new(2) }
        );
        // The acknowledgement of 2 was lost: LispM bases 3 on 1 again.
        assert_eq!(
            replica.receive(Seq::new(1), Seq::new(3), &diff(&a, &screen('c'))),
            Received::Applied { seq: Seq::new(3) }
        );
        assert_eq!(replica.screen(), Some(&screen('c')));
    }

    #[test]
    fn late_and_repeated_frames_are_stale() {
        let mut replica = Replica::new();
        let update = diff(&blank(), &screen('a'));
        replica.receive(Seq::ZERO, Seq::new(5), &update);
        assert_eq!(
            replica.receive(Seq::ZERO, Seq::new(5), &update),
            Received::Stale
        );
        assert_eq!(
            replica.receive(Seq::ZERO, Seq::new(4), &update),
            Received::Stale
        );
    }

    #[test]
    fn a_frame_on_a_state_never_seen_is_unbased() {
        let mut replica = Replica::new();
        assert_eq!(
            replica.receive(Seq::new(7), Seq::new(8), &diff(&blank(), &screen('a'))),
            Received::Unbased
        );
        assert_eq!(replica.screen(), None);
    }
}
