// lispm-remote: the two machines talking.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end scenarios: one responder and its devices exchanging frames,
//! delivered by hand, so a test can drop, duplicate, reorder and relay
//! them under other addresses.

mod pairing;
mod schedules;
mod sessions;

use crate::ChannelId;
use crate::DeviceName;
use crate::ENTROPY_LEN;
use crate::Entropy;
use crate::Fingerprint;
use crate::Frame;
use crate::Identity;
use crate::Initiator;
use crate::KEY_LEN;
use crate::Millis;
use crate::Offer;
use crate::Pairing;
use crate::Payload;
use crate::PeerId;
use crate::Responder;
use crate::StaticSecret;
use crate::device;
use crate::lispm;
use crate::responder::Device;

/// The responder's address.
pub(super) const LISPM: &str = "did:rings:lispm";
/// When the scenarios start.
pub(super) const START: Millis = Millis::new(1_000_000);
/// How long an offer lasts.
pub(super) const OFFER_SPAN: u64 = 120_000;

/// Distinct entropy on every draw.
#[derive(Default)]
pub(super) struct Seeds(u64);

impl Seeds {
    pub(super) fn draw(&mut self) -> Entropy {
        self.0 += 1;
        let mut bytes = [0xA5_u8; ENTROPY_LEN];
        bytes[..8].copy_from_slice(&self.0.to_le_bytes());
        Entropy::new(bytes)
    }
}

pub(super) fn peer(name: &str) -> PeerId {
    PeerId::new(name.as_bytes()).unwrap()
}

pub(super) fn identity(name: &str, secret: u8) -> Identity {
    Identity::new(peer(name), StaticSecret::from_bytes([secret; KEY_LEN])).unwrap()
}

pub(super) fn name(text: &str) -> DeviceName {
    DeviceName::new(text).unwrap()
}

pub(super) fn payload(text: &str) -> Payload {
    Payload::new(text.as_bytes().to_vec()).unwrap()
}

/// LispM: a responder stepped in place.
pub(super) struct Lispm {
    state: Option<Responder>,
    pub(super) now: Millis,
    pub(super) seeds: Seeds,
}

impl Lispm {
    pub(super) fn new(devices: Vec<Device>) -> Self {
        Self {
            state: Some(Responder::new(identity(LISPM, 1), devices)),
            now: START,
            seeds: Seeds::default(),
        }
    }

    pub(super) fn state(&self) -> &Responder {
        self.state.as_ref().unwrap()
    }

    pub(super) fn peer(&self) -> PeerId {
        self.state().identity().peer().clone()
    }

    pub(super) fn step(
        &mut self,
        input: lispm::Input,
    ) -> Result<Vec<lispm::Effect>, lispm::CommandError> {
        let (state, outcome) = self.state.take().unwrap().step(self.now, input);
        self.state = Some(state);
        outcome
    }

    pub(super) fn receive(&mut self, from: &PeerId, frame: Vec<u8>) -> Vec<lispm::Effect> {
        let entropy = self.seeds.draw();
        self.step(lispm::Input::Receive {
            from: from.clone(),
            frame,
            entropy,
        })
        .unwrap()
    }

    /// A new offer's token.
    pub(super) fn offer(&mut self) -> String {
        let expires = Millis::new(self.now.get() + OFFER_SPAN);
        let entropy = self.seeds.draw();
        let effects = self.step(lispm::Input::Offer { expires, entropy }).unwrap();
        effects
            .iter()
            .find_map(|effect| match effect {
                lispm::Effect::ShowOffer(token) => Some(token.expose().to_owned()),
                _ => None,
            })
            .unwrap()
    }

    pub(super) fn tick(&mut self, now: Millis) -> Vec<lispm::Effect> {
        self.now = now;
        self.step(lispm::Input::Tick).unwrap()
    }

    pub(super) fn send(&mut self, to: &PeerId, channel: ChannelId, text: &str) -> Vec<u8> {
        let effects = self
            .step(lispm::Input::Send {
                peer: to.clone(),
                channel,
                payload: payload(text),
            })
            .unwrap();
        single(frames_to(&effects, to))
    }
}

/// A device: an initiator stepped in place.
pub(super) struct Phone {
    state: Option<Initiator>,
    pub(super) peer: PeerId,
    secret: u8,
    pub(super) now: Millis,
    pub(super) seeds: Seeds,
}

impl Phone {
    pub(super) fn new(address: &str, secret: u8, pairing: Option<Pairing>) -> Self {
        Self {
            state: Some(Initiator::new(identity(address, secret), pairing)),
            peer: peer(address),
            secret,
            now: START,
            seeds: Seeds(u64::from(secret) << 32),
        }
    }

    /// The same device after its page reloaded: same key, same pairing.
    pub(super) fn reloaded(&self) -> Self {
        let pairing = self.state().pairing().cloned();
        let mut phone = Self::new(&self.peer.to_string(), self.secret, pairing);
        phone.seeds = Seeds(self.seeds.0 + 1_000);
        phone.now = self.now;
        phone
    }

    pub(super) fn state(&self) -> &Initiator {
        self.state.as_ref().unwrap()
    }

    pub(super) fn step(
        &mut self,
        input: device::Input,
    ) -> Result<Vec<device::Effect>, device::CommandError> {
        let (state, outcome) = self.state.take().unwrap().step(self.now, input);
        self.state = Some(state);
        outcome
    }

    pub(super) fn receive(&mut self, from: &PeerId, frame: Vec<u8>) -> Vec<device::Effect> {
        self.step(device::Input::Receive {
            from: from.clone(),
            frame,
        })
        .unwrap()
    }

    /// Starts pairing with `token`: the PairInit frame and the fingerprint shown.
    pub(super) fn pair(&mut self, token: &str) -> (Vec<u8>, Fingerprint) {
        let offer = Offer::from_token(token).unwrap();
        let entropy = self.seeds.draw();
        let effects = self
            .step(device::Input::Pair {
                offer,
                name: name("Test Phone"),
                entropy,
            })
            .unwrap();
        let fingerprint = effects
            .iter()
            .find_map(|effect| match effect {
                device::Effect::ShowFingerprint(fingerprint) => Some(*fingerprint),
                _ => None,
            })
            .unwrap();
        (single(transmitted(&effects)), fingerprint)
    }

    /// Starts a session: the OpenInit frame.
    pub(super) fn connect(&mut self) -> Vec<u8> {
        let entropy = self.seeds.draw();
        let effects = self.step(device::Input::Connect { entropy }).unwrap();
        single(transmitted(&effects))
    }

    pub(super) fn send(&mut self, text: &str) -> Vec<u8> {
        let effects = self.step(device::Input::Send(payload(text))).unwrap();
        single(transmitted(&effects))
    }
}

pub(super) fn single<T>(mut items: Vec<T>) -> T {
    assert_eq!(items.len(), 1, "expected exactly one item");
    items.pop().unwrap()
}

/// The frames `effects` send to `to`.
pub(super) fn frames_to(effects: &[lispm::Effect], to: &PeerId) -> Vec<Vec<u8>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            lispm::Effect::Transmit {
                to: receiver,
                frame,
            } if receiver == to => Some(frame.clone()),
            _ => None,
        })
        .collect()
}

/// The frames a device's `effects` send, all to LispM.
pub(super) fn transmitted(effects: &[device::Effect]) -> Vec<Vec<u8>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            device::Effect::Transmit { to, frame } => {
                assert_eq!(to, &peer(LISPM));
                Some(frame.clone())
            }
            _ => None,
        })
        .collect()
}

/// The fingerprint LispM asks the owner about.
pub(super) fn question(effects: &[lispm::Effect]) -> Option<Fingerprint> {
    effects.iter().find_map(|effect| match effect {
        lispm::Effect::AskOwner { fingerprint, .. } => Some(*fingerprint),
        _ => None,
    })
}

pub(super) fn paired(effects: &[lispm::Effect]) -> Vec<Device> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            lispm::Effect::Paired(device) => Some(device.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn opened(effects: &[lispm::Effect]) -> Vec<ChannelId> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            lispm::Effect::Opened { channel, .. } => Some(*channel),
            _ => None,
        })
        .collect()
}

pub(super) fn delivered(effects: &[lispm::Effect]) -> Vec<(PeerId, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            lispm::Effect::Delivered { peer, payload, .. } => Some((
                peer.clone(),
                String::from_utf8(payload.as_bytes().to_vec()).unwrap(),
            )),
            _ => None,
        })
        .collect()
}

pub(super) fn phone_delivered(effects: &[device::Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            device::Effect::Delivered(payload) => {
                Some(String::from_utf8(payload.as_bytes().to_vec()).unwrap())
            }
            _ => None,
        })
        .collect()
}

pub(super) fn refusal(frame: &[u8]) -> Option<crate::Refusal> {
    match Frame::decode(frame) {
        Ok(Frame::Refuse { reason, .. }) => Some(reason),
        _ => None,
    }
}

pub(super) fn offer_end(effects: &[lispm::Effect]) -> Option<lispm::OfferEnd> {
    effects.iter().find_map(|effect| match effect {
        lispm::Effect::OfferEnded(end) => Some(*end),
        _ => None,
    })
}

pub(super) fn discarded(effects: &[lispm::Effect]) -> Option<lispm::Discard> {
    effects.iter().find_map(|effect| match effect {
        lispm::Effect::Discarded { why, .. } => Some(*why),
        _ => None,
    })
}

/// Pairs `phone` with `lispm` through a fresh offer, the owner approving;
/// returns the stored record and the open channel.
pub(super) fn pair(lispm: &mut Lispm, phone: &mut Phone) -> (Device, ChannelId) {
    let token = lispm.offer();
    let (init, fingerprint) = phone.pair(&token);
    let asked = lispm.receive(&phone.peer, init);
    assert_eq!(question(&asked), Some(fingerprint));
    let approved = lispm.step(lispm::Input::Approve { fingerprint }).unwrap();
    let record = single(paired(&approved));
    let channel = single(opened(&approved));
    let accept = single(frames_to(&approved, &phone.peer));
    let answer = phone.receive(&lispm.peer(), accept);
    assert!(
        answer
            .iter()
            .any(|e| matches!(e, device::Effect::Paired(_)))
    );
    assert!(
        answer
            .iter()
            .any(|e| matches!(e, device::Effect::Opened(id) if *id == channel))
    );
    (record, channel)
}

/// Opens a session of the paired `phone`; returns its channel.
pub(super) fn connect(lispm: &mut Lispm, phone: &mut Phone) -> ChannelId {
    let init = phone.connect();
    let answered = lispm.receive(&phone.peer, init);
    let channel = single(opened(&answered));
    let accept = frames_to(&answered, &phone.peer)
        .into_iter()
        .find(|frame| matches!(Frame::decode(frame), Ok(Frame::OpenAccept { .. })))
        .unwrap();
    let effects = phone.receive(&lispm.peer(), accept);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Opened(id) if *id == channel))
    );
    channel
}
