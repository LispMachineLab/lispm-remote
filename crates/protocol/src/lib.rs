// lispm-remote: pairing and sessions between LispM and a device.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The protocol of LispM Remote, as two pure state machines.
//!
//! LispM (the [`Responder`]) and a device's page (the [`Initiator`]) reach
//! each other over a transport that addresses peers and authenticates who
//! sent each frame (Rings Network, by DID) but hides nothing from the
//! nodes that relay it.  This crate makes that path private:
//!
//! 1. *Pairing, once.*  LispM shows a QR code holding an [`Offer`]: its
//!    address, its static key, a channel and a one-time pre-shared key.
//!    The device answers with Noise `IKpsk1`, which proves it read the
//!    code; both ends show the same [`Fingerprint`]; LispM pairs only when
//!    the owner approves that fingerprint.  Each end then stores the
//!    other's static key ([`Device`], [`Pairing`]).
//! 2. *Sessions, any time after.*  The device opens a channel with Noise
//!    `KK`, both static keys already known: mutual authentication and fresh
//!    keys every time, so a later compromise does not open earlier
//!    sessions.
//! 3. *Records.*  A channel seals each payload under its own nonce and
//!    accepts each nonce at most once, in any order ([`ReplayWindow`]).
//!
//! # Shape
//!
//! Both machines step as `State → Input → (State, Result<Effects, Error>)`.
//! Time ([`Millis`]) and randomness ([`Entropy`]) are arguments, frames
//! arrive and leave as bytes in effects, and nothing here touches a clock,
//! a random source, a network or a file, so every transition is a
//! function of its state and input and can be replayed.  The shell around
//! a machine performs its effects: sends frames, shows the QR code and
//! questions, stores records.
//!
//! # Trust
//!
//! The transport is trusted for one thing: the sender address it gives
//! with each frame.  The handshakes bind both addresses into their
//! prologue, so that trust is checked rather than assumed.  The page that
//! runs the initiator holds the device's secret key, so the page itself is
//! part of what the owner trusts.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod channel;
mod clock;
mod entropy;
mod ids;
mod initiator;
mod keys;
mod noise;
mod offer;
mod replay;
mod responder;
mod wire;

#[cfg(test)]
mod scenarios;

pub use channel::OpenError;
pub use channel::SealError;
pub use clock::Millis;
pub use entropy::ENTROPY_LEN;
pub use entropy::Entropy;
pub use ids::CHANNEL_LEN;
pub use ids::ChannelId;
pub use ids::DeviceName;
pub use ids::IdError;
pub use ids::MAX_NAME_LEN;
pub use ids::MAX_PEER_LEN;
pub use ids::PeerId;
pub use initiator::Initiator;
pub use initiator::Pairing;
pub use keys::Fingerprint;
pub use keys::Identity;
pub use keys::KEY_LEN;
pub use keys::KeyError;
pub use keys::PublicKey;
pub use keys::StaticSecret;
pub use noise::HandshakeError;
pub use offer::Offer;
pub use offer::OfferError;
pub use offer::OfferToken;
pub use replay::ReplayWindow;
pub use replay::WINDOW;
pub use responder::Device;
pub use responder::Responder;
pub use wire::Body;
pub use wire::Frame;
pub use wire::MAX_PAYLOAD_LEN;
pub use wire::Nonce;
pub use wire::Payload;
pub use wire::Refusal;
pub use wire::VERSION;
pub use wire::WireError;

/// The device's inputs, effects and errors.
pub mod device {
    pub use crate::initiator::CloseReason;
    pub use crate::initiator::CommandError;
    pub use crate::initiator::Discard;
    pub use crate::initiator::Effect;
    pub use crate::initiator::Failure;
    pub use crate::initiator::Input;
    pub use crate::initiator::OPEN_TIMEOUT;
    pub use crate::initiator::RETRY_INTERVAL;
}

/// LispM's inputs, effects and errors.
pub mod lispm {
    pub use crate::responder::CloseReason;
    pub use crate::responder::CommandError;
    pub use crate::responder::Discard;
    pub use crate::responder::Effect;
    pub use crate::responder::Input;
    pub use crate::responder::MAX_CHANNELS_PER_DEVICE;
    pub use crate::responder::MAX_DEVICES;
    pub use crate::responder::OfferEnd;
}
