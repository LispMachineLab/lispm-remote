// lispm-remote: APC strings, taken out of the terminal stream.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The VT parser skips Application Program Commands, which is where the
//! Kitty graphics protocol travels (`ESC _ G … ESC \`).  This splitter
//! takes them out of the stream before the parser sees it:
//!
//! ```text
//!   split(xs ‖ ESC _ p ESC \ ‖ ys) = Pass(xs) · Apc(p) · split(ys)
//! ```
//!
//! The stream arrives in arbitrary chunks, so the splitter is a state
//! machine whose state survives between them: an `ESC` at the end of a
//! chunk waits for the next byte.

/// The longest APC payload kept; longer ones are dropped whole.
pub const MAX_APC_LEN: usize = 16 * 1024 * 1024;

const ESC: u8 = 0x1b;

/// A piece of the stream.
#[derive(Debug, PartialEq, Eq)]
pub enum Segment {
    /// Bytes for the VT parser.
    Pass(Vec<u8>),
    /// An APC's payload, between `ESC _` and `ESC \`.
    Apc(Vec<u8>),
}

#[derive(Debug, Default)]
enum State {
    /// Passing bytes through.
    #[default]
    Ground,
    /// Saw `ESC` outside an APC.
    Escape,
    /// Inside an APC.
    Apc,
    /// Saw `ESC` inside an APC.
    ApcEscape,
}

/// The splitter's state between chunks.
#[derive(Debug, Default)]
pub struct Splitter {
    state: State,
    payload: Vec<u8>,
    overflowed: bool,
}

impl Splitter {
    /// The segments of `chunk`, in order.
    pub fn split(&mut self, chunk: &[u8]) -> Vec<Segment> {
        let mut segments = Vec::new();
        let mut pass = Vec::with_capacity(chunk.len());
        for &byte in chunk {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Escape;
                    } else {
                        pass.push(byte);
                    }
                }
                State::Escape => match byte {
                    b'_' => {
                        flush(&mut segments, &mut pass);
                        self.state = State::Apc;
                        self.payload.clear();
                        self.overflowed = false;
                    }
                    // The first escape passes; the second waits for its byte.
                    ESC => pass.push(ESC),
                    _ => {
                        pass.extend_from_slice(&[ESC, byte]);
                        self.state = State::Ground;
                    }
                },
                State::Apc => {
                    if byte == ESC {
                        self.state = State::ApcEscape;
                    } else {
                        self.keep(byte);
                    }
                }
                State::ApcEscape => {
                    if byte == b'\\' {
                        self.state = State::Ground;
                        if !self.overflowed {
                            segments.push(Segment::Apc(std::mem::take(&mut self.payload)));
                        }
                    } else {
                        self.keep(ESC);
                        self.keep(byte);
                        self.state = State::Apc;
                    }
                }
            }
        }
        flush(&mut segments, &mut pass);
        segments
    }

    fn keep(&mut self, byte: u8) {
        if self.payload.len() < MAX_APC_LEN {
            self.payload.push(byte);
        } else {
            self.overflowed = true;
        }
    }
}

fn flush(segments: &mut Vec<Segment>, pass: &mut Vec<u8>) {
    if !pass.is_empty() {
        segments.push(Segment::Pass(std::mem::take(pass)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The segments with neighbouring passes merged: what a consumer
    /// sees, whatever the chunking.
    fn joined(segments: Vec<Segment>) -> Vec<Segment> {
        let mut merged: Vec<Segment> = Vec::new();
        for segment in segments {
            match (merged.last_mut(), segment) {
                (Some(Segment::Pass(before)), Segment::Pass(bytes)) => before.extend(bytes),
                (_, segment) => merged.push(segment),
            }
        }
        merged
    }

    #[test]
    fn an_apc_is_taken_out_and_the_rest_passes() {
        let mut splitter = Splitter::default();
        let segments = splitter.split(b"ab\x1b_Gi=1\x1b\\cd\x1b[1m");
        assert_eq!(
            segments,
            vec![
                Segment::Pass(b"ab".to_vec()),
                Segment::Apc(b"Gi=1".to_vec()),
                Segment::Pass(b"cd\x1b[1m".to_vec()),
            ]
        );
    }

    #[test]
    fn an_escape_inside_an_apc_that_ends_nothing_is_kept() {
        let mut splitter = Splitter::default();
        let segments = joined(splitter.split(b"\x1b_a\x1bb\x1b\\"));
        assert_eq!(segments, vec![Segment::Apc(b"a\x1bb".to_vec())]);
    }

    #[test]
    fn two_escapes_pass_as_two() {
        let mut splitter = Splitter::default();
        let segments = joined(splitter.split(b"\x1b\x1b[A"));
        assert_eq!(segments, vec![Segment::Pass(b"\x1b\x1b[A".to_vec())]);
    }

    proptest! {
        // Law: the segments are the same however the stream is cut.
        #[test]
        fn chunking_does_not_change_the_segments(
            parts in proptest::collection::vec(
                prop_oneof![
                    Just(b"\x1b_Gq=2\x1b\\".to_vec()),
                    Just(b"\x1b[31m".to_vec()),
                    Just(b"\x1b".to_vec()),
                    proptest::collection::vec(any::<u8>(), 0..8),
                ],
                0..12,
            ),
            cut in any::<usize>(),
        ) {
            let stream: Vec<u8> = parts.concat();
            let whole = joined(Splitter::default().split(&stream));
            let at = if stream.is_empty() { 0 } else { cut % stream.len() };
            let (left, right) = stream.split_at(at);
            let mut splitter = Splitter::default();
            let mut segments = splitter.split(left);
            segments.extend(splitter.split(right));
            prop_assert_eq!(joined(segments), whole);
        }
    }
}
