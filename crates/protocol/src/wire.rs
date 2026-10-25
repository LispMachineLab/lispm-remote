// lispm-remote: frames as bytes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The wire codec.  A frame is
//!
//! ```text
//!   frame   = version:u8 ‖ kind:u8 ‖ channel:[u8; 16] ‖ body
//!   body    = noise                      (PairInit, PairAccept, OpenInit, OpenAccept)
//!           | nonce:u64be ‖ sealed       (Record; sealed = AEAD(plain), ≥ 16 bytes)
//!           | reason:u8                  (Refuse)
//!   plain   = 0x00 ‖ payload | 0x01      (Data, Close)
//! ```
//!
//! Law: `decode(encode(f)) = Ok(f)` for every frame `f`, and `decode` is
//! total: any byte string yields a frame or a [`WireError`], never a panic.
//!
//! Law (versions): every version of the protocol starts its frames with
//! `version:u8 ‖ kind:u8 ‖ channel:[u8; 16]`.  So an end that does not
//! speak a frame's version still reads which version it is
//! ([`WireError::Version`]) and on which channel ([`channel_of`]), and
//! answers on that channel in its own version: the answer's first byte
//! tells the other end which version this one speaks.  A change an older
//! end cannot follow takes the next [`VERSION`]; the Rings namespace the
//! frames travel under stays.

use crate::ids::CHANNEL_LEN;
use crate::ids::ChannelId;

/// The protocol version every frame starts with.
pub const VERSION: u8 = 1;

/// The channel a frame of any version names: its bytes 2 to 17, or none
/// for bytes too short to name one.
#[must_use]
pub fn channel_of(bytes: &[u8]) -> Option<ChannelId> {
    bytes
        .get(2..)
        .and_then(<[u8]>::first_chunk::<CHANNEL_LEN>)
        .map(|channel| ChannelId::from_bytes(*channel))
}
/// The longest Noise message.
pub const MAX_NOISE_LEN: usize = 65_535;
/// The length of an AEAD tag.
pub const TAG_LEN: usize = 16;
/// The longest payload one record carries.
pub const MAX_PAYLOAD_LEN: usize = MAX_NOISE_LEN - TAG_LEN - 1;

const NONCE_LEN: usize = 8;

/// The counter a record is sealed under, unique per direction of a channel.
///
/// Law: `Copy` is sound because a nonce is a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nonce(u64);

impl Nonce {
    /// The nonce `n`.
    #[must_use]
    pub const fn new(n: u64) -> Self {
        Self(n)
    }

    /// The counter's value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An enum whose variants travel as one byte each: `code` and `from_code`
/// are generated from one table, so they are inverse by construction.
macro_rules! byte_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident ($unknown:path) {
            $($(#[$vmeta:meta])* $variant:ident = $code:literal,)+
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            const fn code(self) -> u8 {
                match self {
                    $(Self::$variant => $code,)+
                }
            }

            const fn from_code(code: u8) -> Result<Self, WireError> {
                match code {
                    $($code => Ok(Self::$variant),)+
                    other => Err($unknown(other)),
                }
            }
        }
    };
}

byte_enum! {
    /// Why a responder turned a device away.  A hint for the device's
    /// screen: the transport authenticates who sent it, nothing more.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Refusal (WireError::Refusal) {
        /// The responder holds no pairing for this device.
        NotPaired = 1,
        /// The offer is gone: used, expired, withdrawn or replaced.
        OfferUnavailable = 2,
        /// The owner refused the device.
        OwnerRefused = 3,
        /// Two devices proved the offer's key, so it was dropped.
        Contested = 4,
        /// The responder does not speak the version of the device's
        /// frame; this frame's own first byte says which it speaks.
        OtherVersion = 5,
    }
}

/// One frame between two peers.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A device's first pairing message (Noise IKpsk1, message 1).
    PairInit {
        /// The offer's channel.
        channel: ChannelId,
        /// The Noise message.
        noise: Vec<u8>,
    },
    /// LispM's answer once the owner approved (Noise IKpsk1, message 2).
    PairAccept {
        /// The offer's channel.
        channel: ChannelId,
        /// The Noise message.
        noise: Vec<u8>,
    },
    /// A paired device opening a session (Noise KK, message 1).
    OpenInit {
        /// The new channel.
        channel: ChannelId,
        /// The Noise message.
        noise: Vec<u8>,
    },
    /// LispM's answer (Noise KK, message 2).
    OpenAccept {
        /// The new channel.
        channel: ChannelId,
        /// The Noise message.
        noise: Vec<u8>,
    },
    /// A sealed [`Body`] on an open channel.
    Record {
        /// The channel.
        channel: ChannelId,
        /// The counter it is sealed under.
        nonce: Nonce,
        /// The ciphertext and tag.
        sealed: Vec<u8>,
    },
    /// LispM turning a device away.
    Refuse {
        /// The channel the device asked for.
        channel: ChannelId,
        /// Why.
        reason: Refusal,
    },
}

byte_enum! {
    /// The kinds of frame, as their tag bytes.
    #[derive(Clone, Copy)]
    enum Kind (WireError::Kind) {
        PairInit = 1,
        PairAccept = 2,
        OpenInit = 3,
        OpenAccept = 4,
        Record = 5,
        Refuse = 6,
    }
}

impl Frame {
    /// The channel the frame belongs to.
    #[must_use]
    pub const fn channel(&self) -> ChannelId {
        match self {
            Self::PairInit { channel, .. }
            | Self::PairAccept { channel, .. }
            | Self::OpenInit { channel, .. }
            | Self::OpenAccept { channel, .. }
            | Self::Record { channel, .. }
            | Self::Refuse { channel, .. } => *channel,
        }
    }

    /// The frame's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let kind = match self {
            Self::PairInit { .. } => Kind::PairInit,
            Self::PairAccept { .. } => Kind::PairAccept,
            Self::OpenInit { .. } => Kind::OpenInit,
            Self::OpenAccept { .. } => Kind::OpenAccept,
            Self::Record { .. } => Kind::Record,
            Self::Refuse { .. } => Kind::Refuse,
        };
        let mut bytes = vec![VERSION, kind.code()];
        bytes.extend_from_slice(self.channel().as_bytes());
        match self {
            Self::PairInit { noise, .. }
            | Self::PairAccept { noise, .. }
            | Self::OpenInit { noise, .. }
            | Self::OpenAccept { noise, .. } => bytes.extend_from_slice(noise),
            Self::Record { nonce, sealed, .. } => {
                bytes.extend_from_slice(&nonce.get().to_be_bytes());
                bytes.extend_from_slice(sealed);
            }
            Self::Refuse { reason, .. } => bytes.push(reason.code()),
        }
        bytes
    }

    /// The frame `bytes` encode.
    ///
    /// # Errors
    ///
    /// A [`WireError`] naming the first rule `bytes` break.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let (&version, rest) = bytes.split_first().ok_or(WireError::Truncated)?;
        if version != VERSION {
            return Err(WireError::Version(version));
        }
        let (&tag, rest) = rest.split_first().ok_or(WireError::Truncated)?;
        let kind = Kind::from_code(tag)?;
        let (channel, body) = rest
            .split_first_chunk::<CHANNEL_LEN>()
            .ok_or(WireError::Truncated)?;
        let channel = ChannelId::from_bytes(*channel);
        match kind {
            Kind::PairInit => noise(body).map(|noise| Self::PairInit { channel, noise }),
            Kind::PairAccept => noise(body).map(|noise| Self::PairAccept { channel, noise }),
            Kind::OpenInit => noise(body).map(|noise| Self::OpenInit { channel, noise }),
            Kind::OpenAccept => noise(body).map(|noise| Self::OpenAccept { channel, noise }),
            Kind::Record => record(channel, body),
            Kind::Refuse => match body {
                [code] => Refusal::from_code(*code).map(|reason| Self::Refuse { channel, reason }),
                [] => Err(WireError::Truncated),
                _ => Err(WireError::Trailing),
            },
        }
    }
}

fn noise(body: &[u8]) -> Result<Vec<u8>, WireError> {
    if body.is_empty() {
        Err(WireError::Truncated)
    } else if body.len() > MAX_NOISE_LEN {
        Err(WireError::Oversized)
    } else {
        Ok(body.to_vec())
    }
}

fn record(channel: ChannelId, body: &[u8]) -> Result<Frame, WireError> {
    let (nonce, sealed) = body
        .split_first_chunk::<NONCE_LEN>()
        .ok_or(WireError::Truncated)?;
    if sealed.len() < TAG_LEN {
        Err(WireError::Truncated)
    } else if sealed.len() > MAX_NOISE_LEN {
        Err(WireError::Oversized)
    } else {
        Ok(Frame::Record {
            channel,
            nonce: Nonce(u64::from_be_bytes(*nonce)),
            sealed: sealed.to_vec(),
        })
    }
}

/// What a record carries: an application payload, or the end of the
/// channel.  Sealed, so the end of a channel is as authentic as its data.
#[derive(Debug, PartialEq, Eq)]
pub enum Body {
    /// Application bytes.
    Data(Payload),
    /// The sender closes the channel.
    Close,
}

impl Body {
    pub(crate) fn encode(&self) -> Vec<u8> {
        match self {
            Self::Data(payload) => {
                let mut bytes = Vec::with_capacity(1 + payload.0.len());
                bytes.push(0);
                bytes.extend_from_slice(&payload.0);
                bytes
            }
            Self::Close => vec![1],
        }
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        match bytes.split_first() {
            Some((0, payload)) => Ok(Self::Data(Payload(payload.to_vec()))),
            Some((1, [])) => Ok(Self::Close),
            Some((1, _)) => Err(WireError::Trailing),
            Some((other, _)) => Err(WireError::Body(*other)),
            None => Err(WireError::Truncated),
        }
    }
}

/// Application bytes that fit one record.
///
/// Invariant: `len ≤ MAX_PAYLOAD_LEN`.
#[derive(Debug, PartialEq, Eq)]
pub struct Payload(Vec<u8>);

impl Payload {
    /// The payload `bytes`.
    ///
    /// # Errors
    ///
    /// [`WireError::Oversized`] if `bytes` exceed [`MAX_PAYLOAD_LEN`].
    pub fn new(bytes: Vec<u8>) -> Result<Self, WireError> {
        if bytes.len() > MAX_PAYLOAD_LEN {
            Err(WireError::Oversized)
        } else {
            Ok(Self(bytes))
        }
    }

    /// The bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The bytes, owned.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Why bytes are not a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The bytes end before the frame does.
    #[error("the frame is truncated")]
    Truncated,
    /// The frame names another protocol version.
    #[error("protocol version {0} is not 1")]
    Version(u8),
    /// The frame's kind is unknown.
    #[error("frame kind {0} is unknown")]
    Kind(u8),
    /// A refusal's reason is unknown.
    #[error("refusal reason {0} is unknown")]
    Refusal(u8),
    /// A record's body tag is unknown.
    #[error("record body tag {0} is unknown")]
    Body(u8),
    /// Bytes follow the end of the frame.
    #[error("bytes follow the end of the frame")]
    Trailing,
    /// The frame or payload is longer than Noise allows.
    #[error("the frame is longer than Noise allows")]
    Oversized,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn channel() -> impl Strategy<Value = ChannelId> {
        any::<[u8; CHANNEL_LEN]>().prop_map(ChannelId::from_bytes)
    }

    fn noise_bytes() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(any::<u8>(), 1..256)
    }

    fn refusal() -> impl Strategy<Value = Refusal> {
        prop_oneof![
            Just(Refusal::NotPaired),
            Just(Refusal::OfferUnavailable),
            Just(Refusal::OwnerRefused),
            Just(Refusal::Contested),
            Just(Refusal::OtherVersion),
        ]
    }

    fn frame() -> impl Strategy<Value = Frame> {
        prop_oneof![
            (channel(), noise_bytes())
                .prop_map(|(channel, noise)| Frame::PairInit { channel, noise }),
            (channel(), noise_bytes())
                .prop_map(|(channel, noise)| Frame::PairAccept { channel, noise }),
            (channel(), noise_bytes())
                .prop_map(|(channel, noise)| Frame::OpenInit { channel, noise }),
            (channel(), noise_bytes())
                .prop_map(|(channel, noise)| Frame::OpenAccept { channel, noise }),
            (
                channel(),
                any::<u64>(),
                proptest::collection::vec(any::<u8>(), TAG_LEN..256)
            )
                .prop_map(|(channel, nonce, sealed)| Frame::Record {
                    channel,
                    nonce: Nonce(nonce),
                    sealed
                }),
            (channel(), refusal()).prop_map(|(channel, reason)| Frame::Refuse { channel, reason }),
        ]
    }

    proptest! {
        // Law: ∀ f. decode(encode(f)) = Ok(f).
        #[test]
        fn decoding_an_encoded_frame_gives_it_back(f in frame()) {
            prop_assert_eq!(Frame::decode(&f.encode()), Ok(f));
        }

        // Law: decode is total.
        #[test]
        fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = Frame::decode(&bytes);
        }

        // Law: ∀ b. Body::decode(b.encode()) = Ok(b).
        #[test]
        fn decoding_an_encoded_body_gives_it_back(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let body = Body::Data(Payload::new(bytes).unwrap());
            prop_assert_eq!(Body::decode(&body.encode()), Ok(body));
        }
    }

    #[test]
    fn malformed_frames_name_the_rule_they_break() {
        let header = |kind: u8| {
            let mut bytes = vec![VERSION, kind];
            bytes.extend_from_slice(&[0; CHANNEL_LEN]);
            bytes
        };
        assert_eq!(Frame::decode(&[]), Err(WireError::Truncated));
        assert_eq!(Frame::decode(&[2, 1]), Err(WireError::Version(2)));
        assert_eq!(Frame::decode(&[VERSION, 9]), Err(WireError::Kind(9)));
        assert_eq!(Frame::decode(&header(1)), Err(WireError::Truncated));
        let mut short_record = header(5);
        short_record.extend_from_slice(&[0; NONCE_LEN + TAG_LEN - 1]);
        assert_eq!(Frame::decode(&short_record), Err(WireError::Truncated));
        let mut refuse = header(6);
        refuse.push(9);
        assert_eq!(Frame::decode(&refuse), Err(WireError::Refusal(9)));
        refuse.push(1);
        assert_eq!(Frame::decode(&refuse), Err(WireError::Trailing));
        let mut huge = header(3);
        huge.extend(core::iter::repeat_n(0, MAX_NOISE_LEN + 1));
        assert_eq!(Frame::decode(&huge), Err(WireError::Oversized));
    }

    #[test]
    fn a_frame_of_any_version_names_its_channel() {
        let channel = ChannelId::from_bytes([7; CHANNEL_LEN]);
        let ours = Frame::Refuse {
            channel,
            reason: Refusal::OtherVersion,
        }
        .encode();
        assert_eq!(channel_of(&ours), Some(channel));
        // Another version's frame keeps the header, whatever follows it.
        let mut theirs = ours.clone();
        theirs[0] = VERSION + 1;
        theirs.extend_from_slice(b"what a later version adds");
        assert_eq!(Frame::decode(&theirs), Err(WireError::Version(VERSION + 1)));
        assert_eq!(channel_of(&theirs), Some(channel));
        assert_eq!(channel_of(&ours[..CHANNEL_LEN + 1]), None);
        assert_eq!(channel_of(&[]), None);
    }

    #[test]
    fn bodies_are_data_or_close() {
        assert_eq!(Body::decode(&[1]), Ok(Body::Close));
        assert_eq!(Body::decode(&[1, 0]), Err(WireError::Trailing));
        assert_eq!(Body::decode(&[2]), Err(WireError::Body(2)));
        assert_eq!(Body::decode(&[]), Err(WireError::Truncated));
        assert_eq!(
            Payload::new(vec![0; MAX_PAYLOAD_LEN + 1]),
            Err(WireError::Oversized)
        );
    }
}
