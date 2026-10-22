// lispm-remote: what the QR code carries.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A pairing offer: everything a device needs to pair with one LispM, and
//! the only channel between them that no network carries.  Its token is
//!
//! ```text
//!   token = base64url( version:u8 ‖ channel:[u8; 16] ‖ key:[u8; 32]
//!                      ‖ psk:[u8; 32] ‖ expires:u64be ‖ len:u8 ‖ peer )
//! ```
//!
//! The shell puts it after the `#` of the page's URL, which browsers do
//! not send to the server, and the page removes it once read.
//!
//! Law: `Offer::from_token(o.token()) = Ok(o)`.

use core::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use zeroize::Zeroizing;

use crate::clock::Millis;
use crate::entropy::Entropy;
use crate::ids::CHANNEL_LEN;
use crate::ids::ChannelId;
use crate::ids::IdError;
use crate::ids::PeerId;
use crate::keys::KEY_LEN;
use crate::keys::Psk;
use crate::keys::PublicKey;
use crate::wire::VERSION;

/// A one-time invitation to pair with a responder.
///
/// No `Clone`: the offer holds its pre-shared key, and exactly one
/// responder state and one device hold it.
#[derive(Debug)]
pub struct Offer {
    responder: PeerId,
    responder_key: PublicKey,
    channel: ChannelId,
    psk: Psk,
    expires: Millis,
}

impl Offer {
    /// A fresh offer from `responder`, keyed `responder_key`, valid until
    /// `expires`, its channel and pre-shared key derived from `entropy`.
    pub(crate) fn generate(
        responder: PeerId,
        responder_key: PublicKey,
        expires: Millis,
        entropy: &Entropy,
    ) -> Self {
        Self {
            responder,
            responder_key,
            channel: ChannelId::generate(entropy),
            psk: Psk::generate(entropy),
            expires,
        }
    }

    /// The responder's transport address.
    #[must_use]
    pub const fn responder(&self) -> &PeerId {
        &self.responder
    }

    /// The responder's static public key.
    #[must_use]
    pub const fn responder_key(&self) -> &PublicKey {
        &self.responder_key
    }

    /// The channel the pairing runs on.
    #[must_use]
    pub const fn channel(&self) -> ChannelId {
        self.channel
    }

    /// When the offer stops being honoured.
    #[must_use]
    pub const fn expires(&self) -> Millis {
        self.expires
    }

    /// Whether the offer has expired at `now`.
    #[must_use]
    pub fn expired_at(&self, now: Millis) -> bool {
        now.reached(self.expires)
    }

    /// Whether a frame on `channel` answers this offer.
    #[must_use]
    pub fn names(&self, channel: ChannelId) -> bool {
        self.channel == channel
    }

    pub(crate) const fn psk(&self) -> &Psk {
        &self.psk
    }

    /// The token the QR code shows.
    #[must_use]
    pub fn token(&self) -> OfferToken {
        let peer = self.responder.as_bytes();
        let mut bytes = Zeroizing::new(Vec::with_capacity(TOKEN_FIXED_LEN + peer.len()));
        bytes.push(VERSION);
        bytes.extend_from_slice(self.channel.as_bytes());
        bytes.extend_from_slice(self.responder_key.as_bytes());
        bytes.extend_from_slice(self.psk.as_bytes());
        bytes.extend_from_slice(&self.expires.get().to_be_bytes());
        bytes.push(self.responder.len_byte());
        bytes.extend_from_slice(peer);
        OfferToken(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice())))
    }

    /// The offer a token carries.
    ///
    /// # Errors
    ///
    /// An [`OfferError`] naming the first rule the token breaks.
    pub fn from_token(token: &str) -> Result<Self, OfferError> {
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(token)
                .map_err(|_| OfferError::Encoding)?,
        );
        let (&version, rest) = bytes.split_first().ok_or(OfferError::Truncated)?;
        if version != VERSION {
            return Err(OfferError::Version(version));
        }
        let (channel, rest) = rest
            .split_first_chunk::<CHANNEL_LEN>()
            .ok_or(OfferError::Truncated)?;
        let (key, rest) = rest
            .split_first_chunk::<KEY_LEN>()
            .ok_or(OfferError::Truncated)?;
        let (psk, rest) = rest
            .split_first_chunk::<KEY_LEN>()
            .ok_or(OfferError::Truncated)?;
        let (expires, rest) = rest.split_first_chunk::<8>().ok_or(OfferError::Truncated)?;
        let (&len, peer) = rest.split_first().ok_or(OfferError::Truncated)?;
        if usize::from(len) != peer.len() {
            return Err(OfferError::PeerLength);
        }
        Ok(Self {
            responder: PeerId::new(peer).map_err(OfferError::Peer)?,
            responder_key: PublicKey::from_bytes(*key),
            channel: ChannelId::from_bytes(*channel),
            psk: Psk::from_bytes(*psk),
            expires: Millis::new(u64::from_be_bytes(*expires)),
        })
    }
}

const TOKEN_FIXED_LEN: usize = 1 + CHANNEL_LEN + KEY_LEN + KEY_LEN + 8 + 1;

/// An offer's text form, secret until shown to the owner's device.
pub struct OfferToken(Zeroizing<String>);

impl OfferToken {
    /// The token's text, for the QR code and nothing else.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OfferToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OfferToken(..)")
    }
}

/// Why a token is not an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OfferError {
    /// The token is not base64url.
    #[error("the token is not base64url")]
    Encoding,
    /// The token ends early.
    #[error("the token is truncated")]
    Truncated,
    /// The token names another protocol version.
    #[error("token version {0} is not 1")]
    Version(u8),
    /// The responder's address does not have the length the token says.
    #[error("the responder's address has the wrong length")]
    PeerLength,
    /// The responder's address is not valid.
    #[error("the responder's address is invalid: {0}")]
    Peer(IdError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::ENTROPY_LEN;
    use proptest::prelude::*;

    fn offer(peer: &[u8], expires: u64, seed: u8) -> Offer {
        Offer::generate(
            PeerId::new(peer).unwrap(),
            PublicKey::from_bytes([seed; KEY_LEN]),
            Millis::new(expires),
            &Entropy::new([seed; ENTROPY_LEN]),
        )
    }

    fn same_offer(a: &Offer, b: &Offer) -> bool {
        a.responder == b.responder
            && a.responder_key == b.responder_key
            && a.channel == b.channel
            && a.psk.as_bytes() == b.psk.as_bytes()
            && a.expires == b.expires
    }

    proptest! {
        // Law: ∀ o. from_token(token(o)) = Ok(o).
        #[test]
        fn a_token_gives_its_offer_back(
            peer in proptest::collection::vec(any::<u8>(), 1..=255),
            expires in any::<u64>(),
            seed in any::<u8>(),
        ) {
            let original = offer(&peer, expires, seed);
            let decoded = Offer::from_token(original.token().expose()).unwrap();
            prop_assert!(same_offer(&original, &decoded));
        }

        // Law: from_token is total.
        #[test]
        fn decoding_arbitrary_text_never_panics(text in ".{0,200}") {
            let _ = Offer::from_token(&text);
        }
    }

    #[test]
    fn truncated_and_padded_tokens_are_refused() {
        let token = offer(b"did:rings:emacs", 1, 1).token();
        let text = token.expose();
        let short = text.get(..text.len() - 4).unwrap();
        assert!(Offer::from_token(short).is_err());
        let mut bytes = URL_SAFE_NO_PAD.decode(text).unwrap();
        bytes.push(0);
        let long = URL_SAFE_NO_PAD.encode(&bytes);
        assert_eq!(Offer::from_token(&long).err(), Some(OfferError::PeerLength));
        assert_eq!(Offer::from_token("!!").err(), Some(OfferError::Encoding));
    }

    #[test]
    fn an_offer_expires_at_its_deadline() {
        let offer = offer(b"did:rings:emacs", 100, 1);
        assert!(!offer.expired_at(Millis::new(99)));
        assert!(offer.expired_at(Millis::new(100)));
    }

    #[test]
    fn tokens_do_not_print() {
        let token = offer(b"did:rings:emacs", 1, 1).token();
        assert_eq!(format!("{token:?}"), "OfferToken(..)");
        assert!(!format!("{:?}", offer(b"x", 1, 1)).contains("psk: Psk(["));
    }
}
