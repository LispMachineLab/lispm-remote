// lispm-remote: one client's copy of one surface.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the relay knows of a client's copy of a surface, and the frames
//! that bring it up to date over a channel that may lose, repeat and
//! reorder them (a Rings channel does all three).  This is state
//! synchronisation, as in mosh: every frame goes from the newest state
//! the client acknowledged to what the surface shows *now*, so a lost
//! frame is superseded by the next one, and a slow client skips the
//! states in between.  The client keeps the states it applied
//! ([`lispm_remote_display::Replica`]).
//!
//! ```text
//!   State   = (sent : Seq ⇀ Screen, acked, latest, version, sent_at)
//!   due(now, v)  = (v ≠ version ∧ latest − acked < IN_FLIGHT) ∨ whole ∨ overdue(now)
//!   overdue(now) = latest > acked ∧ now − sent_at ≥ RESEND
//!   frames(now, v, screen), when due:
//!     the parts of diff(sent[acked] or a blank screen, screen), the first
//!     based on acked and each next on the one before; sent[seq] ← the
//!     state each makes; sent_at ← now
//!   ack(n):  n > acked ∧ n ∈ dom sent → acked ← n ; forget sent before n
//!   resync:  acked ← 0 ; sent ← ∅ ; whole ← ⊤     (cleared by the next frames)
//!
//!   Law: whatever frames and acknowledgements were lost, repeated or
//!        reordered, once both get through again the client holds the
//!        surface's screen.
//! ```

use std::collections::BTreeMap;

use lispm_remote_display::MAX_MESSAGE_LEN;
use lispm_remote_display::Screen;
use lispm_remote_display::Seq;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::SurfaceId;
use lispm_remote_display::Update;
use lispm_remote_display::apply;
use lispm_remote_display::diff;
use lispm_remote_display::split;

/// How many frames may wait for the client's acknowledgement before
/// a change waits too.
pub const IN_FLIGHT: u64 = 4;
/// How long, in milliseconds, before unacknowledged frames are superseded
/// by a fresh one.
pub const RESEND_MS: u64 = 400;
/// The most sent states kept.
const MAX_SENT: usize = 64;
/// How many cells of new rows one frame carries at first.
const CELLS_PER_FRAME: usize = 2048;

/// One client's copy of one surface.
#[derive(Debug)]
pub struct Follower {
    surface: SurfaceId,
    sent: BTreeMap<Seq, Screen>,
    acked: Seq,
    latest: Seq,
    version: u64,
    sent_at: u64,
    /// The client asked for the whole screen and has not been sent it.
    whole: bool,
}

impl Follower {
    /// A follower of `surface` whose client has nothing yet.
    #[must_use]
    pub const fn new(surface: SurfaceId) -> Self {
        Self {
            surface,
            sent: BTreeMap::new(),
            acked: Seq::ZERO,
            latest: Seq::ZERO,
            version: 0,
            sent_at: 0,
            whole: false,
        }
    }

    /// The client applied the frame that made state `seq`.
    pub fn ack(&mut self, seq: Seq) {
        if seq > self.acked && self.sent.contains_key(&seq) {
            self.acked = seq;
            self.sent.retain(|kept, _| *kept >= seq);
        }
    }

    /// The client lacks a frame's base: the next frames carry the whole
    /// screen.
    pub fn resync(&mut self) {
        self.acked = Seq::ZERO;
        self.sent.clear();
        self.whole = true;
    }

    fn unacknowledged(&self) -> u64 {
        self.latest.get().saturating_sub(self.acked.get())
    }

    fn overdue(&self, now: u64) -> bool {
        self.latest > self.acked && now.saturating_sub(self.sent_at) >= RESEND_MS
    }

    /// Whether frames are due at `now` (milliseconds) for the surface's
    /// screen at `version`.
    #[must_use]
    pub fn due(&self, now: u64, version: u64) -> bool {
        let fresh = version != self.version && self.unacknowledged() < IN_FLIGHT;
        fresh || self.whole || self.overdue(now)
    }

    /// The frames that bring the client to `screen`, the surface's state
    /// at `version`, at `now`; none when none are due.
    pub fn frames(&mut self, now: u64, version: u64, screen: &Screen) -> Vec<ServerMessage> {
        if !self.due(now, version) {
            return Vec::new();
        }
        let base_screen = match self.sent.get(&self.acked) {
            Some(acked) => acked.clone(),
            None => {
                self.acked = Seq::ZERO;
                Screen::blank(screen.size())
            }
        };
        let update = diff(&base_screen, screen);
        self.version = version;
        self.sent_at = now;
        self.whole = false;
        let settled = self.latest == self.acked && self.acked != Seq::ZERO;
        if settled && unchanged(&base_screen, &update) {
            return Vec::new();
        }
        let parts = fitted(&update);
        let last = parts.len().saturating_sub(1);
        let mut frames = Vec::with_capacity(parts.len());
        let (mut base, mut state) = (self.acked, base_screen);
        for (index, part) in parts.into_iter().enumerate() {
            let Ok(next) = apply(&state, &part) else {
                break;
            };
            let seq = self.latest.next();
            self.sent.insert(seq, next.clone());
            frames.push(ServerMessage::Frame {
                surface: self.surface,
                base,
                seq,
                more: index < last,
                update: part,
            });
            (self.latest, base, state) = (seq, seq, next);
        }
        self.forget_extra();
        frames
    }

    /// Keeps at most [`MAX_SENT`] states, never the acknowledged one.
    fn forget_extra(&mut self) {
        while self.sent.len() > MAX_SENT {
            let oldest = self.sent.keys().copied().find(|seq| *seq != self.acked);
            match oldest {
                Some(seq) => self.sent.remove(&seq),
                None => break,
            };
        }
    }
}

/// Whether `update` changes nothing of `sent`.
fn unchanged(sent: &Screen, update: &Update) -> bool {
    update.rows.is_empty()
        && update.moves.is_empty()
        && update.placements.is_none()
        && update.size == sent.size()
        && update.cursor == sent.cursor
        && update.title == sent.title
}

/// `update` in parts whose messages fit [`MAX_MESSAGE_LEN`]: split by
/// cells first, and a part still too long split again a row at a time.
fn fitted(update: &Update) -> Vec<Update> {
    split(update, CELLS_PER_FRAME)
        .into_iter()
        .flat_map(|part| {
            let message = ServerMessage::Frame {
                surface: SurfaceId::new(0),
                base: Seq::ZERO,
                seq: Seq::ZERO,
                more: false,
                update: part,
            };
            let fits = message.encode().len() <= MAX_MESSAGE_LEN;
            let ServerMessage::Frame { update: part, .. } = message else {
                return Vec::new();
            };
            if fits { vec![part] } else { split(&part, 1) }
        })
        .collect()
}

#[cfg(test)]
mod tests;
