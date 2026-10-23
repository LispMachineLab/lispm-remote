// lispm-remote: the follower, over a channel that loses.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::VecDeque;
use std::sync::Arc;

use lispm_remote_display::Cell;
use lispm_remote_display::Content;
use lispm_remote_display::MAX_MESSAGE_LEN;
use lispm_remote_display::Received;
use lispm_remote_display::Replica;
use lispm_remote_display::Row;
use lispm_remote_display::Screen;
use lispm_remote_display::Seq;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::Size;
use lispm_remote_display::Style;
use lispm_remote_display::SurfaceId;
use proptest::prelude::*;

use super::Follower;
use super::IN_FLIGHT;
use super::RESEND_MS;

fn screen(cols: u16, letters: &[char]) -> Screen {
    let size = Size::new(cols, u16::try_from(letters.len()).unwrap()).unwrap();
    let mut screen = Screen::blank(size);
    for (index, &letter) in (0..).zip(letters) {
        let cells = vec![
            Cell {
                content: Content::Char(letter),
                style: Style::PLAIN
            };
            usize::from(cols)
        ];
        screen.set_row(index, Arc::new(Row::new(cells))).unwrap();
    }
    screen
}

/// What the client says back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reply {
    Ack(Seq),
    Resync,
}

/// The client: a replica, answering each frame as the page does.
fn deliver(replica: &mut Replica, frame: &ServerMessage) -> Option<Reply> {
    let ServerMessage::Frame {
        base, seq, update, ..
    } = frame
    else {
        return None;
    };
    match replica.receive(*base, *seq, update) {
        Received::Applied { seq } => Some(Reply::Ack(seq)),
        Received::Stale => None,
        Received::Unbased => Some(Reply::Resync),
    }
}

fn answer(follower: &mut Follower, reply: Reply) {
    match reply {
        Reply::Ack(seq) => follower.ack(seq),
        Reply::Resync => follower.resync(),
    }
}

#[test]
fn the_first_frame_is_the_whole_screen_and_then_only_changes() {
    let mut follower = Follower::new(SurfaceId::new(1));
    let a = screen(3, &['a', 'b']);
    let frames = follower.frames(0, 1, &a);
    assert!(matches!(
        frames[..],
        [ServerMessage::Frame {
            base: Seq::ZERO,
            ..
        }]
    ));
    follower.ack(Seq::new(1));
    assert!(follower.frames(0, 1, &a).is_empty(), "no change, no frame");
    let frames = follower.frames(0, 2, &screen(3, &['a', 'c']));
    let [ServerMessage::Frame { base, update, .. }] = &frames[..] else {
        panic!("{frames:?}")
    };
    assert_eq!(*base, Seq::new(1));
    assert_eq!(update.rows.len(), 1);
}

#[test]
fn a_lost_frame_is_superseded_by_the_next() {
    let mut follower = Follower::new(SurfaceId::new(1));
    let mut replica = Replica::new();
    let first = follower.frames(0, 1, &screen(2, &['a']));
    let reply = deliver(&mut replica, &first[0]).unwrap();
    answer(&mut follower, reply);
    // Frame 2 is lost; frame 3 is still based on 1, which the client has.
    let _lost = follower.frames(10, 2, &screen(2, &['b']));
    let next = follower.frames(20, 3, &screen(2, &['c']));
    assert_eq!(
        deliver(&mut replica, &next[0]),
        Some(Reply::Ack(Seq::new(3)))
    );
    assert_eq!(replica.screen(), Some(&screen(2, &['c'])));
}

#[test]
fn a_silent_client_gets_no_more_than_the_window_until_frames_are_overdue() {
    let mut follower = Follower::new(SurfaceId::new(1));
    let mut sent = 0;
    for version in 1..=10 {
        let letter = char::from(b'a' + u8::try_from(version).unwrap());
        sent += follower.frames(1, version, &screen(2, &[letter])).len();
    }
    assert_eq!(sent, usize::try_from(IN_FLIGHT).unwrap());
    assert!(
        follower
            .frames(RESEND_MS, 10, &screen(2, &['k']))
            .is_empty(),
        "not yet overdue"
    );
    assert_eq!(
        follower.frames(RESEND_MS + 1, 10, &screen(2, &['k'])).len(),
        1,
        "overdue: sent again"
    );
}

#[test]
fn a_long_frame_is_split_to_fit_a_message() {
    let mut follower = Follower::new(SurfaceId::new(1));
    let size = Size::new(300, 200).unwrap();
    let mut big = Screen::blank(size);
    for row in 0..200_u16 {
        let cells = (0..300).map(|col| Cell {
            content: Content::Char(
                char::from_u32(0x4e00 + (u32::from(row) * 300 + col) % 20_000).unwrap(),
            ),
            style: Style::PLAIN,
        });
        big.set_row(row, Arc::new(Row::new(cells.collect())))
            .unwrap();
    }
    let frames = follower.frames(0, 1, &big);
    assert!(frames.len() > 1);
    assert!(
        frames
            .iter()
            .all(|frame| frame.encode().len() <= MAX_MESSAGE_LEN)
    );
    let mut replica = Replica::new();
    for frame in &frames {
        assert!(matches!(deliver(&mut replica, frame), Some(Reply::Ack(_))));
    }
    assert_eq!(replica.screen(), Some(&big));
}

/// One step of a schedule.
#[derive(Clone, Debug)]
enum Step {
    /// The surface shows these rows now.
    Change(Vec<char>),
    /// Time passes and LispM sends what is due.
    Tick(u64),
    /// The channel hands over the frame at this index (mod its length),
    /// keeping a copy when `again` (a repeat later).
    Frame(usize, bool),
    /// The channel loses the frame at this index.
    LoseFrame(usize),
    /// The channel hands over the reply at this index.
    Reply(usize),
    /// The channel loses the reply at this index.
    LoseReply(usize),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => proptest::collection::vec(proptest::char::range('a', 'd'), 1..4).prop_map(Step::Change),
        3 => (0_u64..300).prop_map(Step::Tick),
        4 => (any::<usize>(), any::<bool>()).prop_map(|(index, again)| Step::Frame(index, again)),
        1 => any::<usize>().prop_map(Step::LoseFrame),
        3 => any::<usize>().prop_map(Step::Reply),
        1 => any::<usize>().prop_map(Step::LoseReply),
    ]
}

fn take<T>(queue: &mut VecDeque<T>, index: usize) -> Option<T> {
    if queue.is_empty() {
        None
    } else {
        queue.remove(index % queue.len())
    }
}

proptest! {
    // Law: whatever the channel lost, repeated or reordered, once frames
    // and replies get through again the client holds the surface's screen.
    #[test]
    fn the_client_converges_whatever_the_channel_did(steps in proptest::collection::vec(step(), 1..60)) {
        let mut follower = Follower::new(SurfaceId::new(1));
        let mut replica = Replica::new();
        let mut frames: VecDeque<ServerMessage> = VecDeque::new();
        let mut replies: VecDeque<Reply> = VecDeque::new();
        let (mut now, mut version, mut shown) = (0_u64, 1_u64, screen(2, &['a']));
        for step in steps {
            match step {
                Step::Change(letters) => {
                    shown = screen(2, &letters);
                    version += 1;
                }
                Step::Tick(elapsed) => {
                    now += elapsed;
                    frames.extend(follower.frames(now, version, &shown));
                }
                Step::Frame(index, again) => {
                    if let Some(frame) = take(&mut frames, index) {
                        replies.extend(deliver(&mut replica, &frame));
                        if again {
                            frames.push_back(frame);
                        }
                    }
                }
                Step::LoseFrame(index) => drop(take(&mut frames, index)),
                Step::Reply(index) => {
                    if let Some(reply) = take(&mut replies, index) {
                        answer(&mut follower, reply);
                    }
                }
                Step::LoseReply(index) => drop(take(&mut replies, index)),
            }
        }
        // The channel recovers: everything gets through, in order.
        for _ in 0..20 {
            now += RESEND_MS;
            frames.extend(follower.frames(now, version, &shown));
            while let Some(frame) = frames.pop_front() {
                replies.extend(deliver(&mut replica, &frame));
            }
            while let Some(reply) = replies.pop_front() {
                answer(&mut follower, reply);
            }
        }
        prop_assert_eq!(replica.screen(), Some(&shown));
    }
}
