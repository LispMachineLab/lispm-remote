// lispm-remote: moments as the shell's clock reports them.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time enters the state machines as a value: every step is given `now`,
//! so a transition depends on the clock only through its argument.

use core::time::Duration;

/// A moment, in milliseconds since the Unix epoch.
///
/// Law: `Copy` is sound because a moment is a number; duplicating it
/// grants nothing and hides no state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Millis(u64);

impl Millis {
    /// The moment `millis` milliseconds after the Unix epoch.
    #[must_use]
    pub const fn new(millis: u64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The moment `span` after this one, or the last representable moment.
    #[must_use]
    pub fn after(self, span: Duration) -> Self {
        let span = u64::try_from(span.as_millis()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(span))
    }

    /// Whether this moment is at or past `deadline`.
    #[must_use]
    pub fn reached(self, deadline: Self) -> bool {
        self >= deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_reaches_itself_and_every_earlier_deadline() {
        let now = Millis::new(1_000);
        assert!(now.reached(now));
        assert!(now.reached(Millis::new(999)));
        assert!(!now.reached(Millis::new(1_001)));
    }

    #[test]
    fn after_saturates_at_the_last_moment() {
        let end = Millis::new(u64::MAX - 1).after(Duration::from_secs(10));
        assert_eq!(end, Millis::new(u64::MAX));
    }
}
