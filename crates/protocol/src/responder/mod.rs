// lispm-remote: LispM's side.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The responder runs in LispM.  It holds at most one pairing offer, the
//! paired devices, and their open channels:
//!
//! ```text
//!   State    = (devices : Peer ⇀ (Device, Channel-id ⇀ Channel),
//!               pending : None | Waiting(offer) | Claimed(offer, claim))
//!   Init     = (restored devices, no channels, None)
//!
//!   Offer(t)           pending ← Waiting(new offer until t)      [old one ends: Replaced]
//!   PairInit from p    Waiting(o) ∧ p proves psk(o)   → Claimed(o, p), ask the owner
//!                      Claimed(o, p) ∧ p again         → Claimed(o, p'), ask again
//!                      Claimed(o, q) ∧ p ≠ q proves it → None, refuse both    [Contested]
//!   Approve(f)         Claimed(o, c) ∧ f = fp(c)      → None, devices[c.peer] ← c, channel open
//!   Refuse | Withdraw  → None, the claimant refused
//!   Tick               expired offer → None
//!   OpenInit from p    p ∈ devices ∧ KK verifies      → channel open (oldest evicted past the cap)
//!   Pair/OpenInit again  the frame that opened (p, id) → its answer again, until p's first record
//!   Record from p      channel (p, id) opens it        → delivered, or closed
//!   Forget(p)          devices ← devices ∖ {p}, its channels closed
//! ```
//!
//! Safety, witnessed by the scenarios:
//!
//! - *Authority*: `Paired(d)` comes only from `Approve(f)` with `f` the
//!   fingerprint of `d`'s claim, and that claim proved the offer's key.
//! - *Single use*: an offer pairs at most one device; a second device
//!   proving its key ends it with no pairing.
//! - *Ownership*: a frame from `p` reaches only channels keyed `(p, _)`;
//!   channels exist only under paired devices (the type nests them).
//! - *Bounds*: `|devices| ≤ MAX_DEVICES`, `|channels(p)| ≤ MAX_CHANNELS_PER_DEVICE`.
//!
//! A step is `State → Input → (State, Result<Effects, CommandError>)`.  A
//! command the state does not permit is refused with the state unchanged;
//! a frame never is: a frame that does not fit is discarded as an effect.

use std::collections::BTreeMap;

use snow::HandshakeState;

use crate::channel::Channel;
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
use crate::wire::Nonce;
use crate::wire::Payload;
use crate::wire::Refusal;

/// The most devices one responder keeps.
pub const MAX_DEVICES: usize = 32;
/// The most open channels one device has at a time.
pub const MAX_CHANNELS_PER_DEVICE: usize = 4;

mod vocabulary;

pub use vocabulary::CloseReason;
pub use vocabulary::CommandError;
pub use vocabulary::Device;
pub use vocabulary::Discard;
pub use vocabulary::Effect;
pub use vocabulary::Input;
pub use vocabulary::OfferEnd;

/// The responder's state.
pub struct Responder {
    identity: Identity,
    devices: BTreeMap<PeerId, Paired>,
    pending: Option<Pending>,
    opened: u64,
}

/// A paired device and its open channels.
struct Paired {
    record: Device,
    channels: BTreeMap<ChannelId, Live>,
}

/// An open channel, when it opened, in opening order, and the answer
/// that opened it while the device may still need it again.
struct Live {
    channel: Channel,
    opened: u64,
    answer: Option<Answer>,
}

/// The frame that opened a channel and the frame that answered it.
///
/// The network may lose the answer, and then the device sends its frame
/// again.  A second handshake would give the device keys the channel does
/// not have, so the first answer is sent again instead.  It is kept until
/// the device's first record shows it has the keys.
struct Answer {
    init: Vec<u8>,
    frame: Vec<u8>,
}

impl Answer {
    /// Whether `message` is the frame this answer answered.
    fn answers(&self, message: &[u8]) -> bool {
        self.init == message
    }

    /// The answer, sent to `to`.
    fn send_to(&self, to: &PeerId) -> Effect {
        // Law: an answer is sent as it is, each time; a copy repeats it.
        Effect::Transmit {
            to: to.clone(),
            frame: self.frame.clone(),
        }
    }
}

/// The offer and, once a device proved its key, that device's claim.
///
/// The claim is boxed: it holds a whole handshake state.
enum Pending {
    Waiting(Offer),
    Claimed { offer: Offer, claim: Box<Claim> },
}

/// A device that proved an offer's key, waiting for the owner.
struct Claim {
    from: PeerId,
    key: PublicKey,
    name: DeviceName,
    fingerprint: Fingerprint,
    handshake: HandshakeState,
    message: Vec<u8>,
}

impl Claim {
    /// The claim `from` makes with the first pairing message `message`.
    fn read(
        identity: &Identity,
        offer: &Offer,
        from: &PeerId,
        message: &[u8],
        entropy: &Entropy,
    ) -> Result<Box<Self>, Discard> {
        let spec = Spec {
            role: Role::Responder,
            transcript: Transcript {
                stage: Stage::Pair,
                initiator: from,
                responder: identity.peer(),
                channel: offer.channel(),
            },
            local: identity,
            remote: None,
            psk: Some(offer.psk()),
        };
        let mut handshake = spec.build(entropy).map_err(Discard::Handshake)?;
        let payload = noise::read(&mut handshake, message).map_err(Discard::Handshake)?;
        let name = DeviceName::from_payload(&payload).map_err(Discard::Name)?;
        let key =
            noise::remote_static(&handshake).ok_or(Discard::Handshake(HandshakeError::Rejected))?;
        Ok(Box::new(Self {
            from: from.clone(),
            key,
            name,
            fingerprint: noise::fingerprint(&handshake),
            handshake,
            message: message.to_vec(),
        }))
    }

    /// Whether `other` comes from the same device as this claim.
    fn same_claimant(&self, other: &Self) -> bool {
        self.from == other.from && self.key == other.key
    }

    fn question(&self) -> Effect {
        Effect::AskOwner {
            name: self.name.clone(),
            fingerprint: self.fingerprint,
        }
    }
}

impl Pending {
    const fn offer(&self) -> &Offer {
        match self {
            Self::Waiting(offer) | Self::Claimed { offer, .. } => offer,
        }
    }

    /// The effects of ending this offer for `end`, the claimant, if any,
    /// told `refusal`.
    fn end(self, end: OfferEnd, refusal: Refusal) -> Vec<Effect> {
        let channel = self.offer().channel();
        let mut effects = Vec::new();
        if let Self::Claimed { claim, .. } = self {
            effects.push(refusal_frame(claim.from, channel, refusal));
        }
        effects.push(Effect::OfferEnded(end));
        effects
    }
}

/// Send `frame` to `to`.
fn transmit(to: PeerId, frame: &Frame) -> Effect {
    Effect::Transmit {
        to,
        frame: frame.encode(),
    }
}

fn refusal_frame(to: PeerId, channel: ChannelId, reason: Refusal) -> Effect {
    transmit(to, &Frame::Refuse { channel, reason })
}

/// The frame discarded and its sender told why.
fn turned_away(from: PeerId, channel: ChannelId, reason: Refusal, why: Discard) -> Vec<Effect> {
    vec![
        refusal_frame(from.clone(), channel, reason),
        Effect::Discarded { from, why },
    ]
}

impl Responder {
    /// A responder for `identity` that remembers `devices`.
    ///
    /// Devices past [`MAX_DEVICES`] are not restored.
    pub fn new(identity: Identity, devices: impl IntoIterator<Item = Device>) -> Self {
        let devices = devices
            .into_iter()
            .take(MAX_DEVICES)
            .map(|record| {
                let paired = Paired {
                    record,
                    channels: BTreeMap::new(),
                };
                (paired.record.peer.clone(), paired)
            })
            .collect();
        Self {
            identity,
            devices,
            pending: None,
            opened: 0,
        }
    }

    /// Who this responder is.
    #[must_use]
    pub const fn identity(&self) -> &Identity {
        &self.identity
    }

    /// The paired devices.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values().map(|paired| &paired.record)
    }

    /// The open channels of the device at `peer`.
    pub fn channels(&self, peer: &PeerId) -> impl Iterator<Item = ChannelId> {
        self.devices
            .get(peer)
            .into_iter()
            .flat_map(|paired| paired.channels.keys().copied())
    }

    /// The current offer.
    #[must_use]
    pub fn offer(&self) -> Option<&Offer> {
        self.pending.as_ref().map(Pending::offer)
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
            Input::Receive {
                from,
                frame,
                entropy,
            } => Ok(self.receive(now, from, &frame, &entropy)),
            Input::Offer { expires, entropy } => self.make_offer(now, expires, &entropy),
            Input::Approve { fingerprint } => self.approve(now, fingerprint),
            Input::Refuse => self.end_claim(),
            Input::Withdraw => self
                .pending
                .take()
                .map(|pending| pending.end(OfferEnd::Withdrawn, Refusal::OfferUnavailable))
                .ok_or(CommandError::NoOffer),
            Input::Send {
                peer,
                channel,
                payload,
            } => self.send(peer, channel, payload),
            Input::Close { peer, channel } => self.close(&peer, channel),
            Input::Forget { peer } => self.forget(peer),
            Input::Tick => Ok(self.tick(now)),
        }
    }

    fn receive(
        &mut self,
        now: Millis,
        from: PeerId,
        bytes: &[u8],
        entropy: &Entropy,
    ) -> Vec<Effect> {
        match Frame::decode(bytes) {
            Err(error) => vec![Effect::Discarded {
                from,
                why: Discard::Malformed(error),
            }],
            Ok(Frame::PairInit { channel, noise }) => {
                self.claim(now, from, channel, &noise, entropy)
            }
            Ok(Frame::OpenInit { channel, noise }) => self.open(from, channel, &noise, entropy),
            Ok(Frame::Record {
                channel,
                nonce,
                sealed,
            }) => self.record(from, channel, nonce, &sealed),
            Ok(Frame::PairAccept { .. } | Frame::OpenAccept { .. } | Frame::Refuse { .. }) => {
                vec![Effect::Discarded {
                    from,
                    why: Discard::Unexpected,
                }]
            }
        }
    }

    fn make_offer(
        &mut self,
        now: Millis,
        expires: Millis,
        entropy: &Entropy,
    ) -> Result<Vec<Effect>, CommandError> {
        if now.reached(expires) {
            return Err(CommandError::InvalidExpiry);
        }
        let mut effects = self
            .pending
            .take()
            .map(|old| old.end(OfferEnd::Replaced, Refusal::OfferUnavailable))
            .unwrap_or_default();
        let offer = Offer::generate(
            self.identity.peer().clone(),
            *self.identity.public_key(),
            expires,
            entropy,
        );
        effects.push(Effect::ShowOffer(offer.token()));
        self.pending = Some(Pending::Waiting(offer));
        Ok(effects)
    }

    /// The answer that opened `from`'s `channel`, to send again, if
    /// `message` is the frame it answered.
    fn answer_again(&self, from: &PeerId, channel: ChannelId, message: &[u8]) -> Option<Effect> {
        let answer = self
            .devices
            .get(from)?
            .channels
            .get(&channel)?
            .answer
            .as_ref()?;
        answer.answers(message).then(|| answer.send_to(from))
    }

    fn claim(
        &mut self,
        now: Millis,
        from: PeerId,
        channel: ChannelId,
        message: &[u8],
        entropy: &Entropy,
    ) -> Vec<Effect> {
        if let Some(again) = self.answer_again(&from, channel, message) {
            // Paired already; the device did not get the answer.
            return vec![again];
        }
        let Some(pending) = self.pending.take() else {
            return turned_away(from, channel, Refusal::OfferUnavailable, Discard::NoOffer);
        };
        if !pending.offer().names(channel) {
            self.pending = Some(pending);
            return turned_away(from, channel, Refusal::OfferUnavailable, Discard::NoOffer);
        }
        if pending.offer().expired_at(now) {
            let mut effects = pending.end(OfferEnd::Expired, Refusal::OfferUnavailable);
            effects.extend(turned_away(
                from,
                channel,
                Refusal::OfferUnavailable,
                Discard::Expired,
            ));
            return effects;
        }
        let claim = match Claim::read(&self.identity, pending.offer(), &from, message, entropy) {
            Ok(claim) => claim,
            Err(why) => {
                self.pending = Some(pending);
                return vec![Effect::Discarded { from, why }];
            }
        };
        match pending {
            Pending::Waiting(offer) => self.await_owner(offer, claim),
            Pending::Claimed {
                offer,
                claim: prior,
            } if prior.same_claimant(&claim) => self.await_owner(offer, claim),
            Pending::Claimed { claim: prior, .. } => vec![
                refusal_frame(prior.from, channel, Refusal::Contested),
                refusal_frame(claim.from, channel, Refusal::Contested),
                Effect::OfferEnded(OfferEnd::Contested),
            ],
        }
    }

    fn await_owner(&mut self, offer: Offer, claim: Box<Claim>) -> Vec<Effect> {
        let question = claim.question();
        self.pending = Some(Pending::Claimed { offer, claim });
        vec![question]
    }

    /// Why the owner may not approve `fingerprint` at `now`, if they may not.
    fn approval_refused(&self, now: Millis, fingerprint: Fingerprint) -> Option<CommandError> {
        match &self.pending {
            Some(Pending::Claimed { offer, claim }) => {
                if claim.fingerprint != fingerprint {
                    Some(CommandError::StaleApproval)
                } else if offer.expired_at(now) {
                    Some(CommandError::Expired)
                } else if !self.has_room_for(&claim.from) {
                    Some(CommandError::TooManyDevices)
                } else {
                    None
                }
            }
            Some(Pending::Waiting(_)) | None => Some(CommandError::NoClaim),
        }
    }

    /// Whether a device at `peer` may be stored.
    fn has_room_for(&self, peer: &PeerId) -> bool {
        self.devices.contains_key(peer) || self.devices.len() < MAX_DEVICES
    }

    fn approve(
        &mut self,
        now: Millis,
        fingerprint: Fingerprint,
    ) -> Result<Vec<Effect>, CommandError> {
        if let Some(error) = self.approval_refused(now, fingerprint) {
            return Err(error);
        }
        let Some(Pending::Claimed { offer, claim }) = self.pending.take() else {
            return Err(CommandError::NoClaim);
        };
        let Claim {
            from,
            key,
            name,
            mut handshake,
            message,
            ..
        } = *claim;
        let channel = offer.channel();
        let finished = noise::write(&mut handshake, &[])
            .and_then(|accept| noise::finish(handshake).map(|open| (accept, open)));
        let Ok((accept, open)) = finished else {
            return Ok(vec![
                refusal_frame(from, channel, Refusal::OfferUnavailable),
                Effect::OfferEnded(OfferEnd::Failed),
            ]);
        };
        let record = Device::new(from, key, name, now);
        let peer = record.peer.clone();
        let answer = Answer {
            init: message,
            frame: Frame::PairAccept {
                channel,
                noise: accept,
            }
            .encode(),
        };
        let mut effects = vec![
            answer.send_to(&peer),
            Effect::Paired(record.clone()),
            Effect::OfferEnded(OfferEnd::Used),
        ];
        if let Some(old) = self.devices.remove(&peer) {
            effects.extend(farewells(&peer, old.channels, CloseReason::Repaired));
        }
        let mut paired = Paired {
            record,
            channels: BTreeMap::new(),
        };
        let opened = self.next_opening();
        paired.channels.insert(
            channel,
            Live {
                channel: open,
                opened,
                answer: Some(answer),
            },
        );
        self.devices.insert(peer.clone(), paired);
        effects.push(Effect::Opened { peer, channel });
        Ok(effects)
    }

    fn end_claim(&mut self) -> Result<Vec<Effect>, CommandError> {
        match self.pending.take() {
            Some(claimed @ Pending::Claimed { .. }) => {
                Ok(claimed.end(OfferEnd::Refused, Refusal::OwnerRefused))
            }
            other => {
                self.pending = other;
                Err(CommandError::NoClaim)
            }
        }
    }

    fn open(
        &mut self,
        from: PeerId,
        channel: ChannelId,
        message: &[u8],
        entropy: &Entropy,
    ) -> Vec<Effect> {
        let Some(paired) = self.devices.get(&from) else {
            return turned_away(from, channel, Refusal::NotPaired, Discard::NotPaired);
        };
        if paired.channels.contains_key(&channel) {
            return match self.answer_again(&from, channel, message) {
                Some(again) => vec![again],
                None => vec![Effect::Discarded {
                    from,
                    why: Discard::Duplicate,
                }],
            };
        }
        let spec = Spec {
            role: Role::Responder,
            transcript: Transcript {
                stage: Stage::Open,
                initiator: &from,
                responder: self.identity.peer(),
                channel,
            },
            local: &self.identity,
            remote: Some(&paired.record.key),
            psk: None,
        };
        let answered = spec.build(entropy).and_then(|mut handshake| {
            noise::read(&mut handshake, message)?;
            let accept = noise::write(&mut handshake, &[])?;
            noise::finish(handshake).map(|open| (accept, open))
        });
        let (accept, open) = match answered {
            Ok(answered) => answered,
            Err(error) => {
                return vec![Effect::Discarded {
                    from,
                    why: Discard::Handshake(error),
                }];
            }
        };
        let opened = self.next_opening();
        let answer = Answer {
            init: message.to_vec(),
            frame: Frame::OpenAccept {
                channel,
                noise: accept,
            }
            .encode(),
        };
        let mut effects = Vec::new();
        let send = answer.send_to(&from);
        if let Some(paired) = self.devices.get_mut(&from) {
            effects.extend(paired.make_room(&from));
            paired.channels.insert(
                channel,
                Live {
                    channel: open,
                    opened,
                    answer: Some(answer),
                },
            );
        }
        effects.push(send);
        effects.push(Effect::Opened {
            peer: from,
            channel,
        });
        effects
    }

    fn record(
        &mut self,
        from: PeerId,
        channel: ChannelId,
        nonce: Nonce,
        sealed: &[u8],
    ) -> Vec<Effect> {
        let Some(paired) = self.devices.get_mut(&from) else {
            return vec![Effect::Discarded {
                from,
                why: Discard::NotPaired,
            }];
        };
        let Some(live) = paired.channels.get_mut(&channel) else {
            return vec![Effect::Discarded {
                from,
                why: Discard::UnknownChannel,
            }];
        };
        let opened = live.channel.open(nonce, sealed);
        if opened.is_ok() {
            // The device sealed a record: it has the channel's keys.
            live.answer = None;
        }
        match opened {
            Ok(Body::Data(payload)) => vec![Effect::Delivered {
                peer: from,
                channel,
                payload,
            }],
            Ok(Body::Close) => {
                paired.channels.remove(&channel);
                vec![Effect::Closed {
                    peer: from,
                    channel,
                    why: CloseReason::ByPeer,
                }]
            }
            Err(error) => vec![Effect::Discarded {
                from,
                why: Discard::Record(error),
            }],
        }
    }

    fn send(
        &mut self,
        peer: PeerId,
        channel: ChannelId,
        payload: Payload,
    ) -> Result<Vec<Effect>, CommandError> {
        let live = self
            .devices
            .get_mut(&peer)
            .and_then(|paired| paired.channels.get_mut(&channel))
            .ok_or(CommandError::NoSuchChannel)?;
        let (nonce, sealed) = live
            .channel
            .seal(&Body::Data(payload))
            .map_err(CommandError::Seal)?;
        Ok(vec![record_frame(peer, channel, nonce, sealed)])
    }

    fn close(&mut self, peer: &PeerId, channel: ChannelId) -> Result<Vec<Effect>, CommandError> {
        let live = self
            .devices
            .get_mut(peer)
            .and_then(|paired| paired.channels.remove(&channel))
            .ok_or(CommandError::NoSuchChannel)?;
        Ok(farewell(peer, channel, live, CloseReason::ByUs))
    }

    fn forget(&mut self, peer: PeerId) -> Result<Vec<Effect>, CommandError> {
        let paired = self
            .devices
            .remove(&peer)
            .ok_or(CommandError::NoSuchDevice)?;
        let mut effects = farewells(&peer, paired.channels, CloseReason::Forgotten);
        effects.push(Effect::Forgotten(peer));
        Ok(effects)
    }

    fn tick(&mut self, now: Millis) -> Vec<Effect> {
        match self.pending.take() {
            Some(pending) if pending.offer().expired_at(now) => {
                pending.end(OfferEnd::Expired, Refusal::OfferUnavailable)
            }
            other => {
                self.pending = other;
                Vec::new()
            }
        }
    }

    fn next_opening(&mut self) -> u64 {
        let opened = self.opened;
        self.opened = self.opened.saturating_add(1);
        opened
    }
}

impl Paired {
    /// Closes the oldest channels until one more fits.
    ///
    /// Post: `channels.len() < MAX_CHANNELS_PER_DEVICE`.
    fn make_room(&mut self, peer: &PeerId) -> Vec<Effect> {
        let mut effects = Vec::new();
        while self.channels.len() >= MAX_CHANNELS_PER_DEVICE {
            let oldest = self
                .channels
                .iter()
                .min_by_key(|(_, live)| live.opened)
                .map(|(id, _)| *id);
            let Some((id, live)) = oldest.and_then(|id| self.channels.remove_entry(&id)) else {
                break;
            };
            effects.extend(farewell(peer, id, live, CloseReason::Evicted));
        }
        effects
    }
}

fn record_frame(to: PeerId, channel: ChannelId, nonce: Nonce, sealed: Vec<u8>) -> Effect {
    transmit(
        to,
        &Frame::Record {
            channel,
            nonce,
            sealed,
        },
    )
}

/// The effects of closing `live`: a sealed close for the device, if one
/// can still be sealed, and the notice for the shell.
fn farewell(peer: &PeerId, channel: ChannelId, mut live: Live, why: CloseReason) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Ok((nonce, sealed)) = live.channel.seal(&Body::Close) {
        effects.push(record_frame(peer.clone(), channel, nonce, sealed));
    }
    effects.push(Effect::Closed {
        peer: peer.clone(),
        channel,
        why,
    });
    effects
}

fn farewells(peer: &PeerId, channels: BTreeMap<ChannelId, Live>, why: CloseReason) -> Vec<Effect> {
    channels
        .into_iter()
        .flat_map(|(channel, live)| farewell(peer, channel, live, why))
        .collect()
}
