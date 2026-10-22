// lispm-remote: the names peers, channels and devices go by.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Validated names.  A [`PeerId`] is the transport's address of an end (a
//! Rings DID, as text); the transport authenticates the sender of every
//! frame, and the protocol binds each handshake to both addresses, so a
//! frame relayed under another address fails its handshake.

use core::fmt;

use crate::entropy::Entropy;
use crate::entropy::Purpose;

/// The most bytes a [`PeerId`] may have.
pub const MAX_PEER_LEN: usize = 255;
/// The most bytes a [`DeviceName`] may have.
pub const MAX_NAME_LEN: usize = 64;
/// The length of a [`ChannelId`].
pub const CHANNEL_LEN: usize = 16;

/// The transport address of one end, as the transport authenticated it.
///
/// Invariant: `1 ≤ len ≤ MAX_PEER_LEN`, so it fits one length byte.
///
/// Law: `Clone` is sound because an address is a name; a copy grants
/// nothing, since authority comes from the transport's authentication.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(Box<[u8]>);

impl PeerId {
    /// The address whose bytes are `bytes`.
    ///
    /// # Errors
    ///
    /// [`IdError::PeerLength`] unless `1 ≤ bytes.len() ≤ MAX_PEER_LEN`.
    pub fn new(bytes: &[u8]) -> Result<Self, IdError> {
        if (1..=MAX_PEER_LEN).contains(&bytes.len()) {
            Ok(Self(bytes.into()))
        } else {
            Err(IdError::PeerLength(bytes.len()))
        }
    }

    /// The address's bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The address's length, which its invariant fits in one byte.
    pub(crate) fn len_byte(&self) -> u8 {
        u8::try_from(self.0.len()).unwrap_or(u8::MAX)
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({self})")
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

/// The identifier of a channel: one session between a device and LispM.
///
/// A pairing's channel is named by its offer; a later session's by the
/// device.  Channels are keyed by peer and identifier together, so equal
/// identifiers of different peers never meet.
///
/// Law: `Copy` is sound because an identifier is a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelId([u8; CHANNEL_LEN]);

impl ChannelId {
    /// The identifier whose bytes are `bytes`.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; CHANNEL_LEN]) -> Self {
        Self(bytes)
    }

    /// The identifier's bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; CHANNEL_LEN] {
        &self.0
    }

    /// A fresh identifier derived from `entropy`.
    pub(crate) fn generate(entropy: &Entropy) -> Self {
        let derived = entropy.derive(Purpose::Channel);
        let mut bytes = [0_u8; CHANNEL_LEN];
        for (out, byte) in bytes.iter_mut().zip(derived.bytes().iter()) {
            *out = *byte;
        }
        Self(bytes)
    }
}

/// What a device calls itself, shown to the owner when it asks to pair.
///
/// Invariant: non-empty UTF-8 of at most [`MAX_NAME_LEN`] bytes with no
/// control characters, so the owner sees exactly what was sent.
///
/// Law: `Clone` is sound because a name is a label.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceName(String);

impl DeviceName {
    /// The name `text`.
    ///
    /// # Errors
    ///
    /// [`IdError::NameLength`] if `text` is empty or longer than
    /// [`MAX_NAME_LEN`] bytes; [`IdError::NameControl`] if it holds a
    /// control character.
    pub fn new(text: &str) -> Result<Self, IdError> {
        if !(1..=MAX_NAME_LEN).contains(&text.len()) {
            Err(IdError::NameLength(text.len()))
        } else if text.chars().any(char::is_control) {
            Err(IdError::NameControl)
        } else {
            Ok(Self(text.to_owned()))
        }
    }

    /// The name read from a handshake payload.
    pub(crate) fn from_payload(bytes: &[u8]) -> Result<Self, IdError> {
        let text = core::str::from_utf8(bytes).map_err(|_| IdError::NameEncoding)?;
        Self::new(text)
    }

    /// The name's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// A peer address must have 1 to 255 bytes.
    #[error("a peer address has {0} bytes, not 1 to 255")]
    PeerLength(usize),
    /// A device name must have 1 to 64 bytes.
    #[error("a device name has {0} bytes, not 1 to 64")]
    NameLength(usize),
    /// A device name must not hold control characters.
    #[error("a device name holds a control character")]
    NameControl,
    /// A device name must be UTF-8.
    #[error("a device name is not UTF-8")]
    NameEncoding,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::ENTROPY_LEN;

    #[test]
    fn peer_ids_fit_one_length_byte() {
        assert_eq!(PeerId::new(b""), Err(IdError::PeerLength(0)));
        assert_eq!(PeerId::new(&[b'a'; 256]), Err(IdError::PeerLength(256)));
        let longest = PeerId::new(&[b'a'; MAX_PEER_LEN]).unwrap();
        assert_eq!(longest.len_byte(), u8::MAX);
    }

    #[test]
    fn device_names_are_short_printable_text() {
        assert!(DeviceName::new("iPhone Safari").is_ok());
        assert_eq!(DeviceName::new(""), Err(IdError::NameLength(0)));
        assert_eq!(DeviceName::new("a\nb"), Err(IdError::NameControl));
        assert_eq!(
            DeviceName::new(&"x".repeat(MAX_NAME_LEN + 1)),
            Err(IdError::NameLength(MAX_NAME_LEN + 1))
        );
        assert_eq!(
            DeviceName::from_payload(&[0xff, 0xfe]),
            Err(IdError::NameEncoding)
        );
    }

    #[test]
    fn channel_ids_follow_their_entropy() {
        let a = ChannelId::generate(&Entropy::new([4; ENTROPY_LEN]));
        let b = ChannelId::generate(&Entropy::new([4; ENTROPY_LEN]));
        let c = ChannelId::generate(&Entropy::new([5; ENTROPY_LEN]));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
