// lispm-remote: the device's side.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The initiator runs in the device's page.  It pairs once with an offer,
//! then opens one channel at a time:
//!
//! ```text
//!   Phase  = Idle | Pairing(offer, hs, wait) | Opening(id, hs, wait) | Open(id, channel)
//!   wait   = (frame, until, retry_at)
//!   Init   = Idle, with the stored pairing if any
//!
//!   Pair(offer)       Idle ∧ offer unexpired → Pairing, send PairInit, show fingerprint
//!   Connect           Idle ∧ paired          → Opening, send OpenInit
//!   PairAccept        Pairing ∧ verifies     → Open, store the pairing
//!   OpenAccept        Opening ∧ verifies     → Open
//!   Refuse(r)         Pairing | Opening      → Idle  (NotPaired also forgets the pairing)
//!   Record            Open                   → delivered, or Idle when closed
//!   Tick              past `until`           → Idle, timed out
//!                     past `retry_at`        → send the waiting frame again
//!   frame of version v ≠ VERSION             → Idle, failed: the responder speaks v
//! ```
//!
//! The network may lose a frame between hops and say nothing, so a phase
//! that waits for an answer sends its frame again every [`RETRY_INTERVAL`]
//! of ticks; the responder answers a repeated frame with the answer it gave
//! the first time.
//!
//! Every frame must come from the responder the phase talks to; the
//! transport authenticates the sender, and the handshake binds it.

use core::time::Duration;

use snow::HandshakeState;

use crate::channel::Channel;
use crate::channel::OpenError;
use crate::channel::SealError;
use crate::clock::Millis;
use crate::entropy::Entropy;
use crate::ids::ChannelId;
use crate::ids::DeviceName;
use crate::ids::PeerId;
use crate::keys::Fingerprint;
use crate::keys::Identity;
use crate::keys::PublicKey;
use crate::noise;
use crate::noise::HandshakeError;
use crate::noise::Role;
use crate::noise::Spec;
use crate::noise::Stage;
use crate::noise::Transcript;
use crate::offer::Offer;
use crate::wire::Body;
use crate::wire::Frame;
use crate::wire::Payload;
use crate::wire::Refusal;
use crate::wire::WireError;

/// How long a device waits for its session to open.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a device waits for an answer before sending its frame again.
pub const RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// The responder a device paired with, as the device stores it.
///
/// Law: `Clone` is sound because a record is data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    responder: PeerId,
    key: PublicKey,
}

impl Pairing {
    /// The pairing with the responder at `responder`, keyed `key`, as the
    /// device restores it.
    #[must_use]
    pub const fn new(responder: PeerId, key: PublicKey) -> Self {
        Self { responder, key }
    }

    /// The responder's transport address.
    #[must_use]
    pub const fn responder(&self) -> &PeerId {
        &self.responder
    }

    /// The responder's static public key.
    #[must_use]
    pub const fn key(&self) -> &PublicKey {
        &self.key
    }
}

/// Something the device needs or does.
#[derive(Debug)]
pub enum Input {
    /// Pair with `offer`, calling this device `name`.
    Pair {
        /// The offer read from the QR code.
        offer: Offer,
        /// What the owner will see this device called.
        name: DeviceName,
        /// Fresh randomness for the handshake.
        entropy: Entropy,
    },
    /// Open a session with the paired responder.
    Connect {
        /// Fresh randomness for the channel and handshake.
        entropy: Entropy,
    },
    /// A frame arrived from `from`, as the transport authenticated it.
    Receive {
        /// The sender.
        from: PeerId,
        /// The frame's bytes.
        frame: Vec<u8>,
    },
    /// Send `payload` on the open channel.
    Send(Payload),
    /// Close the open channel, or stop opening it.
    Close,
    /// Time passed.
    Tick,
}

/// Something the page must do or show.
#[derive(Debug)]
pub enum Effect {
    /// Send `frame` to `to`.
    Transmit {
        /// The receiver.
        to: PeerId,
        /// The frame's bytes.
        frame: Vec<u8>,
    },
    /// Show this fingerprint until pairing ends, for the owner to compare.
    ShowFingerprint(Fingerprint),
    /// Store this pairing, replacing any other.
    Paired(Pairing),
    /// Delete the stored pairing: the responder no longer knows this device.
    Unpaired,
    /// The channel opened.
    Opened(ChannelId),
    /// The channel delivered a payload.
    Delivered(Payload),
    /// The channel closed.
    Closed(CloseReason),
    /// Pairing or opening ended without a channel.
    Failed(Failure),
    /// A frame was discarded.
    Discarded(Discard),
}

/// Why the channel closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The responder closed it.
    ByPeer,
    /// This device closed it.
    ByUs,
}

/// Why pairing or opening failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The responder refused.
    Refused(Refusal),
    /// No answer came in time.
    TimedOut,
    /// The answer did not authenticate.
    Handshake(HandshakeError),
    /// This device stopped waiting.
    Abandoned,
    /// The responder speaks this version of the protocol, which this
    /// device does not.
    OtherVersion(u8),
}

/// Why a frame was discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discard {
    /// It is not a frame.
    Malformed(WireError),
    /// It came from someone this device is not talking to.
    Stranger,
    /// It does not fit the current phase or channel.
    Unexpected,
    /// Its record was not accepted.
    Record(OpenError),
}

/// Why a command was refused.  The state is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// Pairing or a channel is already under way.
    #[error("pairing or a channel is already under way")]
    Busy,
    /// The offer has expired.
    #[error("the offer has expired")]
    OfferExpired,
    /// The device is not paired.
    #[error("the device is not paired")]
    NotPaired,
    /// No channel is open.
    #[error("no channel is open")]
    NotOpen,
    /// This device could not build its handshake.
    #[error("the handshake could not be set up: {0}")]
    Handshake(HandshakeError),
    /// The payload could not be sealed.
    #[error("the payload could not be sealed: {0}")]
    Seal(SealError),
}

/// The device's state.
pub struct Initiator {
    identity: Identity,
    pairing: Option<Pairing>,
    phase: Phase,
}

enum Phase {
    Idle,
    Pairing {
        offer: Offer,
        handshake: HandshakeState,
        wait: Wait,
    },
    Opening {
        channel: ChannelId,
        handshake: HandshakeState,
        wait: Wait,
    },
    Open {
        id: ChannelId,
        channel: Channel,
    },
}

/// A frame waiting for its answer: until when, and when to send it again.
struct Wait {
    to: PeerId,
    frame: Vec<u8>,
    until: Millis,
    retry_at: Millis,
}

impl Wait {
    fn new(now: Millis, to: PeerId, frame: Vec<u8>, until: Millis) -> Self {
        Self {
            to,
            frame,
            until,
            retry_at: now.after(RETRY_INTERVAL),
        }
    }

    /// The frame to send first.
    fn transmit(&self) -> Effect {
        // Law: the frame is sent as it is, each time; a copy repeats it.
        Effect::Transmit {
            to: self.to.clone(),
            frame: self.frame.clone(),
        }
    }
}

impl Phase {
    /// What the phase waits for, if it waits.
    fn wait(&mut self) -> Option<&mut Wait> {
        match self {
            Self::Pairing { wait, .. } | Self::Opening { wait, .. } => Some(wait),
            Self::Idle | Self::Open { .. } => None,
        }
    }
}

impl Initiator {
    /// A device for `identity`, paired as `pairing` says.
    #[must_use]
    pub const fn new(identity: Identity, pairing: Option<Pairing>) -> Self {
        Self {
            identity,
            pairing,
            phase: Phase::Idle,
        }
    }

    /// Who this device is.
    #[must_use]
    pub const fn identity(&self) -> &Identity {
        &self.identity
    }

    /// The responder this device is paired with.
    #[must_use]
    pub const fn pairing(&self) -> Option<&Pairing> {
        self.pairing.as_ref()
    }

    /// Whether a channel is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. })
    }

    /// The state after `input` at `now`, and what follows from it.
    ///
    /// Post: on `Err`, the state is the one given.
    pub fn step(mut self, now: Millis, input: Input) -> (Self, Result<Vec<Effect>, CommandError>) {
        let outcome = self.apply(now, input);
        (self, outcome)
    }

    fn apply(&mut self, now: Millis, input: Input) -> Result<Vec<Effect>, CommandError> {
        match input {
            Input::Pair {
                offer,
                name,
                entropy,
            } => self.pair(now, offer, &name, &entropy),
            Input::Connect { entropy } => self.connect(now, &entropy),
            Input::Receive { from, frame } => Ok(self.receive(&from, &frame)),
            Input::Send(payload) => self.send(payload),
            Input::Close => self.close(),
            Input::Tick => Ok(self.tick(now)),
        }
    }

    /// Whether the device may start pairing or opening.
    const fn idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    fn pair(
        &mut self,
        now: Millis,
        offer: Offer,
        name: &DeviceName,
        entropy: &Entropy,
    ) -> Result<Vec<Effect>, CommandError> {
        if !self.idle() {
            return Err(CommandError::Busy);
        }
        if offer.expired_at(now) {
            return Err(CommandError::OfferExpired);
        }
        let channel = offer.channel();
        let spec = Spec {
            role: Role::Initiator,
            transcript: Transcript {
                stage: Stage::Pair,
                initiator: self.identity.peer(),
                responder: offer.responder(),
                channel,
            },
            local: &self.identity,
            remote: Some(offer.responder_key()),
            psk: Some(offer.psk()),
        };
        let mut handshake = spec.build(entropy).map_err(CommandError::Handshake)?;
        let message = noise::write(&mut handshake, name.as_str().as_bytes())
            .map_err(CommandError::Handshake)?;
        let wait = Wait::new(
            now,
            offer.responder().clone(),
            Frame::PairInit {
                channel,
                noise: message,
            }
            .encode(),
            offer.expires(),
        );
        let effects = vec![
            wait.transmit(),
            Effect::ShowFingerprint(noise::fingerprint(&handshake)),
        ];
        self.phase = Phase::Pairing {
            offer,
            handshake,
            wait,
        };
        Ok(effects)
    }

    fn connect(&mut self, now: Millis, entropy: &Entropy) -> Result<Vec<Effect>, CommandError> {
        if !self.idle() {
            return Err(CommandError::Busy);
        }
        let pairing = self.pairing.as_ref().ok_or(CommandError::NotPaired)?;
        let channel = ChannelId::generate(entropy);
        let spec = Spec {
            role: Role::Initiator,
            transcript: Transcript {
                stage: Stage::Open,
                initiator: self.identity.peer(),
                responder: &pairing.responder,
                channel,
            },
            local: &self.identity,
            remote: Some(&pairing.key),
            psk: None,
        };
        let mut handshake = spec.build(entropy).map_err(CommandError::Handshake)?;
        let message = noise::write(&mut handshake, &[]).map_err(CommandError::Handshake)?;
        let wait = Wait::new(
            now,
            pairing.responder.clone(),
            Frame::OpenInit {
                channel,
                noise: message,
            }
            .encode(),
            now.after(OPEN_TIMEOUT),
        );
        let effects = vec![wait.transmit()];
        self.phase = Phase::Opening {
            channel,
            handshake,
            wait,
        };
        Ok(effects)
    }

    /// The responder the current phase talks to.
    fn counterpart(&self) -> Option<&PeerId> {
        match &self.phase {
            Phase::Pairing { offer, .. } => Some(offer.responder()),
            Phase::Opening { .. } | Phase::Open { .. } => {
                self.pairing.as_ref().map(Pairing::responder)
            }
            Phase::Idle => None,
        }
    }

    fn receive(&mut self, from: &PeerId, bytes: &[u8]) -> Vec<Effect> {
        if self.counterpart() != Some(from) {
            return vec![Effect::Discarded(Discard::Stranger)];
        }
        let frame = match Frame::decode(bytes) {
            Ok(frame) => frame,
            Err(WireError::Version(version)) => return self.other_version(version),
            Err(error) => return vec![Effect::Discarded(Discard::Malformed(error))],
        };
        match (core::mem::replace(&mut self.phase, Phase::Idle), frame) {
            (
                Phase::Pairing {
                    offer, handshake, ..
                },
                Frame::PairAccept { channel, noise },
            ) if offer.names(channel) => self.paired(offer, handshake, &noise),
            (Phase::Pairing { offer, .. }, Frame::Refuse { channel, reason })
                if offer.names(channel) =>
            {
                vec![Effect::Failed(Failure::Refused(reason))]
            }
            (
                Phase::Opening {
                    channel, handshake, ..
                },
                Frame::OpenAccept { channel: id, noise },
            ) if channel == id => self.opened(channel, handshake, &noise),
            (
                Phase::Opening { channel, .. },
                Frame::Refuse {
                    channel: id,
                    reason,
                },
            ) if channel == id => self.refused(reason),
            (
                Phase::Open { id, mut channel },
                Frame::Record {
                    channel: to,
                    nonce,
                    sealed,
                },
            ) if id == to => match channel.open(nonce, &sealed) {
                Ok(Body::Data(payload)) => {
                    self.phase = Phase::Open { id, channel };
                    vec![Effect::Delivered(payload)]
                }
                Ok(Body::Close) => vec![Effect::Closed(CloseReason::ByPeer)],
                Err(error) => {
                    self.phase = Phase::Open { id, channel };
                    vec![Effect::Discarded(Discard::Record(error))]
                }
            },
            (phase, _) => {
                self.phase = phase;
                vec![Effect::Discarded(Discard::Unexpected)]
            }
        }
    }

    fn paired(
        &mut self,
        offer: Offer,
        mut handshake: HandshakeState,
        message: &[u8],
    ) -> Vec<Effect> {
        let finished = noise::read(&mut handshake, message).and_then(|_| noise::finish(handshake));
        match finished {
            Ok(channel) => {
                let pairing = Pairing::new(offer.responder().clone(), *offer.responder_key());
                let id = offer.channel();
                self.pairing = Some(pairing.clone());
                self.phase = Phase::Open { id, channel };
                vec![Effect::Paired(pairing), Effect::Opened(id)]
            }
            Err(error) => vec![Effect::Failed(Failure::Handshake(error))],
        }
    }

    fn opened(
        &mut self,
        id: ChannelId,
        mut handshake: HandshakeState,
        message: &[u8],
    ) -> Vec<Effect> {
        let finished = noise::read(&mut handshake, message).and_then(|_| noise::finish(handshake));
        match finished {
            Ok(channel) => {
                self.phase = Phase::Open { id, channel };
                vec![Effect::Opened(id)]
            }
            Err(error) => vec![Effect::Failed(Failure::Handshake(error))],
        }
    }

    /// The responder's frame came in the protocol's version `version`,
    /// which this device does not speak: what the phase waited for or had
    /// ends, and its frame is not sent again.
    fn other_version(&mut self, version: u8) -> Vec<Effect> {
        let mut effects = Vec::with_capacity(2);
        if let Phase::Open { .. } = core::mem::replace(&mut self.phase, Phase::Idle) {
            effects.push(Effect::Closed(CloseReason::ByPeer));
        }
        effects.push(Effect::Failed(Failure::OtherVersion(version)));
        effects
    }

    fn refused(&mut self, reason: Refusal) -> Vec<Effect> {
        let mut effects = Vec::new();
        if reason == Refusal::NotPaired && self.pairing.take().is_some() {
            effects.push(Effect::Unpaired);
        }
        effects.push(Effect::Failed(Failure::Refused(reason)));
        effects
    }

    fn send(&mut self, payload: Payload) -> Result<Vec<Effect>, CommandError> {
        let Phase::Open { id, channel } = &mut self.phase else {
            return Err(CommandError::NotOpen);
        };
        let to = self.pairing.as_ref().ok_or(CommandError::NotPaired)?;
        let (nonce, sealed) = channel
            .seal(&Body::Data(payload))
            .map_err(CommandError::Seal)?;
        Ok(vec![transmit(
            to.responder.clone(),
            &Frame::Record {
                channel: *id,
                nonce,
                sealed,
            },
        )])
    }

    fn close(&mut self) -> Result<Vec<Effect>, CommandError> {
        match core::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Open { id, mut channel } => {
                let mut effects = Vec::new();
                if let (Some(pairing), Ok((nonce, sealed))) =
                    (&self.pairing, channel.seal(&Body::Close))
                {
                    effects.push(transmit(
                        pairing.responder.clone(),
                        &Frame::Record {
                            channel: id,
                            nonce,
                            sealed,
                        },
                    ));
                }
                effects.push(Effect::Closed(CloseReason::ByUs));
                Ok(effects)
            }
            Phase::Pairing { .. } | Phase::Opening { .. } => {
                Ok(vec![Effect::Failed(Failure::Abandoned)])
            }
            Phase::Idle => Err(CommandError::NotOpen),
        }
    }

    fn tick(&mut self, now: Millis) -> Vec<Effect> {
        let Some(wait) = self.phase.wait() else {
            return Vec::new();
        };
        if now.reached(wait.until) {
            self.phase = Phase::Idle;
            vec![Effect::Failed(Failure::TimedOut)]
        } else if now.reached(wait.retry_at) {
            wait.retry_at = now.after(RETRY_INTERVAL);
            vec![wait.transmit()]
        } else {
            Vec::new()
        }
    }
}

/// Send `frame` to `to`.
fn transmit(to: PeerId, frame: &Frame) -> Effect {
    Effect::Transmit {
        to,
        frame: frame.encode(),
    }
}
