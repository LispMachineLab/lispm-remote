// lispm-remote: a client's input, in the order it was typed.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Input arrives numbered, and a channel may lose, repeat and reorder
//! it; keys must still reach Emacs in the order they were pressed.  The
//! client resends what LispM has not acknowledged, so a gap usually
//! fills; one that stays open past [`GIVE_UP_MS`] is skipped, so a lost
//! key never stops the keys after it.
//!
//! ```text
//!   Order   = (next : u64, waiting : u64 ⇀ (input, arrived))
//!   take(n, i, now):  n < next ∨ n ∈ dom waiting → nothing    (a repeat)
//!                     otherwise waiting[n] ← (i, now) ; release(now)
//!   release(now):     hand over waiting[next], waiting[next+1], … ;
//!                     when the oldest waiting arrived GIVE_UP ago, next ← it, again
//!   start(n, now):    n > next → next ← n ; forget waiting below n ; release(now)
//!   acknowledged = next − 1
//!
//!   Law: whatever the order and repetition of arrivals, the inputs handed
//!        over are in increasing order and each at most once, none
//!        numbered below a start; with no loss, they are all of them.
//! ```

use std::collections::BTreeMap;

/// How long a gap in the input is waited on before it is skipped.
pub const GIVE_UP_MS: u64 = 2000;

/// A client's input, put back in order.
#[derive(Debug)]
pub struct Order<T> {
    next: u64,
    waiting: BTreeMap<u64, (T, u64)>,
}

impl<T> Default for Order<T> {
    fn default() -> Self {
        Self {
            next: 1,
            waiting: BTreeMap::new(),
        }
    }
}

impl<T> Order<T> {
    /// The input numbered `seq` arrived at `now` (milliseconds); returns
    /// what may now be handed over, in order.
    pub fn take(&mut self, seq: u64, input: T, now: u64) -> Vec<T> {
        if seq >= self.next {
            self.waiting.entry(seq).or_insert((input, now));
        }
        self.release(now)
    }

    /// What may be handed over at `now`, gaps that waited too long
    /// skipped.
    pub fn release(&mut self, now: u64) -> Vec<T> {
        let mut out = Vec::new();
        loop {
            while let Some((input, _)) = self.waiting.remove(&self.next) {
                out.push(input);
                self.next += 1;
            }
            let stale = self
                .waiting
                .first_key_value()
                .filter(|(_, (_, arrived))| now.saturating_sub(*arrived) >= GIVE_UP_MS)
                .map(|(seq, _)| *seq);
            match stale {
                Some(seq) => self.next = seq,
                None => return out,
            }
        }
    }

    /// Input numbered below `seq` is of an earlier session and is not
    /// taken: returns what may now be handed over.
    pub fn start(&mut self, seq: u64, now: u64) -> Vec<T> {
        if seq > self.next {
            self.waiting = self.waiting.split_off(&seq);
            self.next = seq;
        }
        self.release(now)
    }

    /// The last input handed over, or skipped: what the client may stop
    /// resending.
    #[must_use]
    pub const fn acknowledged(&self) -> u64 {
        self.next - 1
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn input_out_of_order_is_handed_over_in_order() {
        let mut order = Order::default();
        assert_eq!(order.take(2, 'b', 0), Vec::<char>::new());
        assert_eq!(order.take(3, 'c', 0), Vec::<char>::new());
        assert_eq!(order.take(1, 'a', 0), vec!['a', 'b', 'c']);
        assert_eq!(order.acknowledged(), 3);
        assert_eq!(order.take(2, 'b', 0), Vec::<char>::new(), "a repeat");
    }

    #[test]
    fn a_gap_that_stays_open_is_skipped() {
        let mut order = Order::default();
        assert!(order.take(2, 'b', 0).is_empty());
        assert!(order.release(GIVE_UP_MS - 1).is_empty());
        assert_eq!(order.release(GIVE_UP_MS), vec!['b']);
        assert_eq!(order.acknowledged(), 2);
        assert!(order.take(1, 'a', GIVE_UP_MS).is_empty(), "too late");
    }

    #[test]
    fn input_of_an_earlier_session_is_not_taken() {
        let mut order = Order::default();
        assert!(
            order.take(3, 'x', 0).is_empty(),
            "late, of the last session"
        );
        assert_eq!(order.start(5, 0), Vec::<char>::new());
        assert_eq!(order.acknowledged(), 4);
        assert!(
            order.take(4, 'y', 0).is_empty(),
            "late, of the last session"
        );
        assert_eq!(order.take(5, 'a', 0), vec!['a']);
        assert!(
            order.release(GIVE_UP_MS).is_empty(),
            "nothing old comes out"
        );
    }

    proptest! {
        // Law: handed over in increasing order, each at most once, and
        // all of them when nothing was lost.
        #[test]
        fn input_comes_out_in_order_once(
            arrivals in proptest::collection::vec(1_u64..30, 0..80),
        ) {
            let mut order = Order::default();
            let mut out = Vec::new();
            for &seq in &arrivals {
                out.extend(order.take(seq, seq, 0));
            }
            prop_assert!(out.windows(2).all(|pair| pair[0] < pair[1]));
            let mut all: Vec<u64> = arrivals.clone();
            all.sort_unstable();
            all.dedup();
            let complete = all.iter().zip(1..).all(|(seq, n)| *seq == n);
            if complete {
                prop_assert_eq!(out, all);
            }
        }

        // Law: once a session starts at n, nothing numbered below n is
        // handed over, whatever arrived before or arrives late.
        #[test]
        fn input_below_a_start_never_comes_out(
            before in proptest::collection::vec(1_u64..30, 0..40),
            first in 1_u64..30,
            after in proptest::collection::vec(1_u64..30, 0..40),
        ) {
            let mut order = Order::default();
            for &seq in &before {
                order.take(seq, seq, 0);
            }
            let mut out = order.start(first, 0);
            for &seq in &after {
                out.extend(order.take(seq, seq, 0));
            }
            out.extend(order.release(GIVE_UP_MS));
            prop_assert!(out.iter().all(|seq| *seq >= first));
        }
    }
}
