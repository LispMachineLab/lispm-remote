// lispm-remote: one client's copy of one surface.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the relay knows of a client's copy of a surface, and the frames
//! that bring it up to date.  This is state synchronisation, not a
//! stream: the frames go from what the client has to what the surface
//! shows *now*, so a slow client skips the states in between.
//!
//! ```text
//!   State   = (sent : Option Screen, seq, acked, version)
//!   frames  : State × (version, screen) → State × [Frame]
//!     sent = None        → Frame(base 0, …)   the whole screen
//!     seq − acked ≥ IN_FLIGHT → []            wait for the client
//!     otherwise          → Frame(base seq, diff(sent, screen)), split to fit
//!   ack(n)  : acked ← n           when acked < n ≤ seq
//!   resync  : sent ← None ; acked ← seq
//!
//!   Law: a client that applies every frame, in order, to its copy holds
//!        the screen the last frames were made from.
//! ```

use lispm_remote_display::MAX_MESSAGE_LEN;
use lispm_remote_display::Screen;
use lispm_remote_display::Seq;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::SurfaceId;
use lispm_remote_display::Update;
use lispm_remote_display::diff;
use lispm_remote_display::split;

/// How many frames may wait for the client's acknowledgement.
pub const IN_FLIGHT: u64 = 4;
/// How many cells of new rows one frame carries at first.
const CELLS_PER_FRAME: usize = 2048;

/// One client's copy of one surface.
#[derive(Debug)]
pub struct Follower {
    surface: SurfaceId,
    sent: Option<Screen>,
    seq: Seq,
    acked: Seq,
    version: u64,
}

impl Follower {
    /// A follower of `surface` whose client has nothing yet.
    #[must_use]
    pub const fn new(surface: SurfaceId) -> Self {
        Self {
            surface,
            sent: None,
            seq: Seq::ZERO,
            acked: Seq::ZERO,
            version: 0,
        }
    }

    /// The client applied frames up to `seq`.
    pub fn ack(&mut self, seq: Seq) {
        if seq > self.acked && seq <= self.seq {
            self.acked = seq;
        }
    }

    /// The client lost its copy: the next frame carries the whole screen.
    pub fn resync(&mut self) {
        self.sent = None;
        self.acked = self.seq;
    }

    /// Whether the surface's screen at `version` is newer than the copy.
    #[must_use]
    pub fn behind(&self, version: u64) -> bool {
        self.sent.is_none() || version != self.version
    }

    /// Whether the client must acknowledge before more frames go.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.seq.get().saturating_sub(self.acked.get()) >= IN_FLIGHT
    }

    /// The frames that bring the client's copy to `screen`, the surface's
    /// state at `version`.
    pub fn frames(&mut self, version: u64, screen: &Screen) -> Vec<ServerMessage> {
        if !self.behind(version) || self.waiting() {
            return Vec::new();
        }
        let (mut base, update) = match &self.sent {
            Some(sent) => (self.seq, diff(sent, screen)),
            None => (Seq::ZERO, diff(&Screen::blank(screen.size()), screen)),
        };
        self.version = version;
        if self
            .sent
            .as_ref()
            .is_some_and(|sent| unchanged(sent, &update))
        {
            return Vec::new();
        }
        let parts = fitted(&update);
        let last = parts.len().saturating_sub(1);
        let mut frames = Vec::with_capacity(parts.len());
        for (index, update) in parts.into_iter().enumerate() {
            let seq = self.seq.next();
            frames.push(ServerMessage::Frame {
                surface: self.surface,
                base,
                seq,
                more: index < last,
                update,
            });
            self.seq = seq;
            base = seq;
        }
        self.sent = Some(screen.clone());
        frames
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
mod tests {
    use std::sync::Arc;

    use lispm_remote_display::Cell;
    use lispm_remote_display::Content;
    use lispm_remote_display::Row;
    use lispm_remote_display::Size;
    use lispm_remote_display::Style;
    use lispm_remote_display::apply;
    use proptest::prelude::*;

    use super::*;

    fn screen(cols: u16, letters: &[char]) -> Screen {
        let size = Size::new(cols, u16::try_from(letters.len()).unwrap()).unwrap();
        let mut screen = Screen::blank(size);
        for (index, &letter) in (0..).zip(letters) {
            let row = Row::new(vec![
                Cell {
                    content: Content::Char(letter),
                    style: Style::PLAIN
                };
                usize::from(cols)
            ]);
            screen.set_row(index, Arc::new(row)).unwrap();
        }
        screen
    }

    /// A client: its copy and its sequence number, refusing frames for
    /// another base as the page does.
    struct Client {
        copy: Option<Screen>,
        seq: Seq,
    }

    impl Client {
        fn receive(&mut self, frames: Vec<ServerMessage>) -> bool {
            for frame in frames {
                let ServerMessage::Frame {
                    base, seq, update, ..
                } = frame
                else {
                    return false;
                };
                let copy = match (&self.copy, base) {
                    (_, Seq::ZERO) => Screen::blank(update.size),
                    (Some(copy), base) if base == self.seq => copy.clone(),
                    _ => return false,
                };
                self.copy = Some(apply(&copy, &update).unwrap());
                self.seq = seq;
            }
            true
        }
    }

    #[test]
    fn the_first_frame_is_the_whole_screen_and_then_only_changes() {
        let mut follower = Follower::new(SurfaceId::new(1));
        let a = screen(3, &['a', 'b']);
        let frames = follower.frames(1, &a);
        assert!(matches!(
            frames[..],
            [ServerMessage::Frame {
                base: Seq::ZERO,
                ..
            }]
        ));
        assert!(follower.frames(1, &a).is_empty(), "no change, no frame");
        let b = screen(3, &['a', 'c']);
        let frames = follower.frames(2, &b);
        let [ServerMessage::Frame { base, update, .. }] = &frames[..] else {
            panic!("{frames:?}")
        };
        assert_eq!(*base, Seq::new(1));
        assert_eq!(update.rows.len(), 1);
    }

    #[test]
    fn a_slow_client_is_not_sent_past_the_window_and_skips_states() {
        let mut follower = Follower::new(SurfaceId::new(1));
        let mut sent = 0;
        for version in 1..=10 {
            sent += follower
                .frames(
                    version,
                    &screen(2, &[char::from(b'a' + u8::try_from(version).unwrap())]),
                )
                .len();
        }
        assert_eq!(sent, usize::try_from(IN_FLIGHT).unwrap());
        follower.ack(Seq::new(IN_FLIGHT));
        assert_eq!(follower.frames(10, &screen(2, &['k'])).len(), 1);
    }

    #[test]
    fn a_long_frame_is_split_to_fit_a_message() {
        let mut follower = Follower::new(SurfaceId::new(1));
        let letters: Vec<char> = (0..200)
            .map(|i| char::from_u32(0x4e00 + i).unwrap())
            .collect();
        // 200 rows of 300 distinct wide characters' worth of cells.
        let size = Size::new(300, 200).unwrap();
        let mut big = Screen::blank(size);
        for (index, &letter) in (0..).zip(&letters) {
            let cells = (0..300).map(|col| Cell {
                content: Content::Char(char::from_u32(u32::from(letter) + col).unwrap()),
                style: Style::PLAIN,
            });
            big.set_row(index, Arc::new(Row::new(cells.collect())))
                .unwrap();
        }
        let frames = follower.frames(1, &big);
        assert!(frames.len() > 1);
        assert!(
            frames
                .iter()
                .all(|frame| frame.encode().len() <= MAX_MESSAGE_LEN)
        );
        let mut client = Client {
            copy: None,
            seq: Seq::ZERO,
        };
        assert!(client.receive(frames));
        assert_eq!(client.copy, Some(big));
    }

    proptest! {
        // Law: a client that applies every frame holds the last screen,
        // through acknowledgements and resyncs.
        #[test]
        fn the_client_ends_with_the_last_screen(
            steps in proptest::collection::vec(
                (proptest::collection::vec(proptest::char::range('a', 'd'), 1..5), 0_u8..4),
                1..20,
            ),
        ) {
            let mut follower = Follower::new(SurfaceId::new(1));
            let mut client = Client { copy: None, seq: Seq::ZERO };
            let mut last = None;
            for (version, (letters, action)) in (1..).zip(steps) {
                let next = screen(2, &letters);
                let frames = follower.frames(version, &next);
                if !client.receive(frames) {
                    follower.resync();
                }
                match action {
                    0 => follower.ack(client.seq),
                    1 => {
                        follower.resync();
                        client.copy = None;
                    }
                    _ => {}
                }
                last = Some((version, next));
            }
            let (version, next) = last.unwrap();
            follower.ack(client.seq);
            let frames = follower.frames(version + 1, &next);
            prop_assert!(client.receive(frames));
            prop_assert_eq!(client.copy, Some(next));
        }
    }
}
