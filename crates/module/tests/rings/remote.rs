// lispm-remote: pairing and sessions carried by Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! LispM Remote end to end: LispM's `Responder` on one node, the phone's
//! `Initiator` on another, every frame between them sent and received by
//! the nodes.  The two never exchange an offer; each is connected only to
//! a third node, and the network finds the way between them.
//!
//! What is real: Rings, WebRTC, the module's node and the protocol's
//! machines.  What stands in: the owner (who approves the fingerprint the
//! phone shows), the phone's page (a native node instead of a browser
//! one), the clock and the random source of the steps.

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use lispm_remote_protocol::ChannelId;
use lispm_remote_protocol::Device;
use lispm_remote_protocol::DeviceName;
use lispm_remote_protocol::ENTROPY_LEN;
use lispm_remote_protocol::Entropy;
use lispm_remote_protocol::Fingerprint;
use lispm_remote_protocol::HandshakeError;
use lispm_remote_protocol::Identity;
use lispm_remote_protocol::Initiator;
use lispm_remote_protocol::KEY_LEN;
use lispm_remote_protocol::Millis;
use lispm_remote_protocol::Offer;
use lispm_remote_protocol::Pairing;
use lispm_remote_protocol::Payload;
use lispm_remote_protocol::PeerId;
use lispm_remote_protocol::Responder;
use lispm_remote_protocol::StaticSecret;
use lispm_remote_protocol::device;
use lispm_remote_protocol::lispm;

use crate::support::Peer;
use crate::support::connect;

const LISPM_SECRET: [u8; KEY_LEN] = [1; KEY_LEN];
const PHONE_SECRET: [u8; KEY_LEN] = [7; KEY_LEN];
/// How long an offer lasts.
const OFFER_MILLIS: u64 = 120_000;

fn now() -> Millis {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    Millis::new(u64::try_from(since.as_millis()).unwrap())
}

fn peer_id(did: &str) -> PeerId {
    PeerId::new(did.as_bytes()).unwrap()
}

/// Distinct entropy on every draw: the steps need it distinct, the test
/// does not need it secret.
struct Seeds(u64);

impl Seeds {
    fn draw(&mut self) -> Entropy {
        self.0 += 1;
        let mut bytes = [0x5A_u8; ENTROPY_LEN];
        bytes[..8].copy_from_slice(&self.0.to_le_bytes());
        Entropy::new(bytes)
    }
}

/// LispM: the responder, on a Rings node.
pub struct Lispm {
    pub peer: Peer,
    state: Option<Responder>,
    seeds: Seeds,
}

impl Lispm {
    pub fn new(peer: Peer, devices: Vec<Device>) -> Self {
        let identity =
            Identity::new(peer_id(&peer.did()), StaticSecret::from_bytes(LISPM_SECRET)).unwrap();
        Self {
            state: Some(Responder::new(identity, devices)),
            peer,
            seeds: Seeds(1 << 40),
        }
    }

    fn state(&self) -> &Responder {
        self.state.as_ref().unwrap()
    }

    /// The responder as after Emacs restarted: its stored devices, no
    /// channels.  The node keeps running.
    pub fn restart(&mut self) {
        let devices: Vec<Device> = self.state().devices().cloned().collect();
        let identity = Identity::new(
            peer_id(&self.peer.did()),
            StaticSecret::from_bytes(LISPM_SECRET),
        )
        .unwrap();
        self.state = Some(Responder::new(identity, devices));
    }

    /// Steps `input`, sends what it transmits, returns the other effects.
    pub fn step(&mut self, input: lispm::Input) -> Vec<lispm::Effect> {
        let (state, outcome) = self.state.take().unwrap().step(now(), input);
        self.state = Some(state);
        let mut rest = Vec::new();
        for effect in outcome.unwrap() {
            match effect {
                lispm::Effect::Transmit { to, frame } => self.peer.send(&to.to_string(), frame),
                other => rest.push(other),
            }
        }
        rest
    }

    /// Takes the next frame from `from` off the network and steps it.
    pub fn receive_from(&mut self, from: &str) -> Vec<lispm::Effect> {
        let frame = self.peer.frame_from(from);
        let entropy = self.seeds.draw();
        self.step(lispm::Input::Receive {
            from: peer_id(from),
            frame,
            entropy,
        })
    }

    /// A new offer's token, for the QR code.
    pub fn offer(&mut self) -> String {
        let expires = Millis::new(now().get() + OFFER_MILLIS);
        let entropy = self.seeds.draw();
        self.step(lispm::Input::Offer { expires, entropy })
            .iter()
            .find_map(|effect| match effect {
                lispm::Effect::ShowOffer(token) => Some(token.expose().to_owned()),
                _ => None,
            })
            .unwrap()
    }
}

/// The phone: the initiator, on a Rings node.
pub struct Phone {
    pub peer: Peer,
    state: Option<Initiator>,
    seeds: Seeds,
}

impl Phone {
    pub fn new(peer: Peer, pairing: Option<Pairing>) -> Self {
        let identity =
            Identity::new(peer_id(&peer.did()), StaticSecret::from_bytes(PHONE_SECRET)).unwrap();
        Self {
            state: Some(Initiator::new(identity, pairing)),
            peer,
            seeds: Seeds(2 << 40),
        }
    }

    fn state(&self) -> &Initiator {
        self.state.as_ref().unwrap()
    }

    /// The initiator as after the page reloaded: its stored pairing.
    pub fn restart(&mut self) {
        let pairing = self.state().pairing().cloned();
        let identity = Identity::new(
            peer_id(&self.peer.did()),
            StaticSecret::from_bytes(PHONE_SECRET),
        )
        .unwrap();
        self.state = Some(Initiator::new(identity, pairing));
    }

    /// Steps `input`; returns the frames it would send, unsent, and the
    /// other effects.
    pub fn step_held(
        &mut self,
        input: device::Input,
    ) -> (Vec<(String, Vec<u8>)>, Vec<device::Effect>) {
        let (state, outcome) = self.state.take().unwrap().step(now(), input);
        self.state = Some(state);
        let mut frames = Vec::new();
        let mut rest = Vec::new();
        for effect in outcome.unwrap() {
            match effect {
                device::Effect::Transmit { to, frame } => frames.push((to.to_string(), frame)),
                other => rest.push(other),
            }
        }
        (frames, rest)
    }

    /// Steps `input`, sends what it transmits, returns the other effects.
    pub fn step(&mut self, input: device::Input) -> Vec<device::Effect> {
        let (frames, rest) = self.step_held(input);
        for (to, frame) in frames {
            self.peer.send(&to, frame);
        }
        rest
    }

    pub fn receive_from(&mut self, from: &str) -> Vec<device::Effect> {
        let frame = self.peer.frame_from(from);
        self.step(device::Input::Receive {
            from: peer_id(from),
            frame,
        })
    }

    pub fn pair_input(&mut self, token: &str) -> device::Input {
        device::Input::Pair {
            offer: Offer::from_token(token).unwrap(),
            name: DeviceName::new("Integration Phone").unwrap(),
            entropy: self.seeds.draw(),
        }
    }
}

fn payload(text: &str) -> Payload {
    Payload::new(text.as_bytes().to_vec()).unwrap()
}

fn shown_fingerprint(effects: &[device::Effect]) -> Fingerprint {
    effects
        .iter()
        .find_map(|effect| match effect {
            device::Effect::ShowFingerprint(fingerprint) => Some(*fingerprint),
            _ => None,
        })
        .unwrap()
}

fn asked_fingerprint(effects: &[lispm::Effect]) -> Option<Fingerprint> {
    effects.iter().find_map(|effect| match effect {
        lispm::Effect::AskOwner { fingerprint, .. } => Some(*fingerprint),
        _ => None,
    })
}

/// Pairs `phone` with `lispm` through an offer, the owner approving the
/// fingerprint the phone shows; returns the stored device and the channel.
pub fn pair_over(lispm: &mut Lispm, phone: &mut Phone) -> (Device, ChannelId) {
    let token = lispm.offer();
    let input = phone.pair_input(&token);
    let fingerprint = shown_fingerprint(&phone.step(input));
    let asked = lispm.receive_from(&phone.peer.did());
    assert_eq!(asked_fingerprint(&asked), Some(fingerprint));

    let approved = lispm.step(lispm::Input::Approve { fingerprint });
    let device = approved
        .iter()
        .find_map(|effect| match effect {
            lispm::Effect::Paired(device) => Some(device.clone()),
            _ => None,
        })
        .unwrap();
    let channel = opened(&approved);
    let answer = phone.receive_from(&lispm.peer.did());
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
    (device, channel)
}

fn opened(effects: &[lispm::Effect]) -> ChannelId {
    effects
        .iter()
        .find_map(|effect| match effect {
            lispm::Effect::Opened { channel, .. } => Some(*channel),
            _ => None,
        })
        .unwrap()
}

/// Opens a session of the paired `phone`; returns its channel.
pub fn open_session(lispm: &mut Lispm, phone: &mut Phone) -> ChannelId {
    let entropy = phone.seeds.draw();
    phone.step(device::Input::Connect { entropy });
    let channel = opened(&lispm.receive_from(&phone.peer.did()));
    let answer = phone.receive_from(&lispm.peer.did());
    assert!(
        answer
            .iter()
            .any(|e| matches!(e, device::Effect::Opened(id) if *id == channel))
    );
    channel
}

/// The phone sends `up`, LispM answers `down`, each delivered once.
pub fn exchange(lispm: &mut Lispm, phone: &mut Phone, channel: ChannelId, up: &str, down: &str) {
    phone.step(device::Input::Send(payload(up)));
    let delivered = lispm.receive_from(&phone.peer.did());
    assert!(delivered.iter().any(|effect| matches!(
        effect,
        lispm::Effect::Delivered { payload: p, .. } if p.as_bytes() == up.as_bytes()
    )));

    lispm.step(lispm::Input::Send {
        peer: peer_id(&phone.peer.did()),
        channel,
        payload: payload(down),
    });
    let delivered = phone.receive_from(&lispm.peer.did());
    assert!(delivered.iter().any(|effect| matches!(
        effect,
        device::Effect::Delivered(p) if p.as_bytes() == down.as_bytes()
    )));
}

#[test]
fn pairing_and_sessions_run_over_rings_through_a_relay() {
    let mut relay = Peer::start();
    let mut a = Peer::start();
    let mut b = Peer::start();
    connect(&mut a, &mut relay);
    connect(&mut b, &mut relay);
    let mut lispm = Lispm::new(a, Vec::new());
    let mut phone = Phone::new(b, None);

    let (device, channel) = pair_over(&mut lispm, &mut phone);
    assert_eq!(device.peer(), &peer_id(&phone.peer.did()));
    assert_eq!(device.name().as_str(), "Integration Phone");
    exchange(&mut lispm, &mut phone, channel, "agents?", "coder: idle");

    // Both machines restart from what they stored; the nodes stay.
    lispm.restart();
    phone.restart();
    let channel = open_session(&mut lispm, &mut phone);
    exchange(&mut lispm, &mut phone, channel, "approve 1", "approved");

    relay.node.stop();
}

#[test]
fn a_pairing_frame_resent_by_another_node_fails_its_handshake() {
    let mut relay = Peer::start();
    let mut a = Peer::start();
    let mut b = Peer::start();
    let mut c = Peer::start();
    connect(&mut a, &mut relay);
    connect(&mut b, &mut relay);
    connect(&mut c, &mut relay);
    let mut lispm = Lispm::new(a, Vec::new());
    let mut phone = Phone::new(b, None);

    let token = lispm.offer();
    let input = phone.pair_input(&token);
    let (frames, effects) = phone.step_held(input);
    let (to, frame) = frames.into_iter().next().unwrap();
    assert_eq!(to, lispm.peer.did());

    // C read the frame on its way and sends it on as its own; Rings tells
    // LispM it came from C, and the handshake is bound to the sender.
    c.send(&to, frame.clone());
    let effects_of_c = lispm.receive_from(&c.did());
    assert!(effects_of_c.iter().any(|effect| matches!(
        effect,
        lispm::Effect::Discarded {
            why: lispm::Discard::Handshake(HandshakeError::Rejected),
            ..
        }
    )));
    assert_eq!(asked_fingerprint(&effects_of_c), None);

    // The phone's own frame still pairs.
    phone.peer.send(&to, frame);
    let asked = lispm.receive_from(&phone.peer.did());
    assert_eq!(asked_fingerprint(&asked), Some(shown_fingerprint(&effects)));

    relay.node.stop();
    c.node.stop();
}
