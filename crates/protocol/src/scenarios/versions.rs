// lispm-remote: scenarios of two ends of different versions.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An end of another version of the protocol is a frame whose first byte
//! is not [`VERSION`], with the header every version keeps.  Each end
//! names the other's version at the first such frame, and the device stops
//! there instead of sending its frame again until it times out.

use super::*;
use crate::Refusal;
use crate::VERSION;
use crate::device::Failure;
use crate::device::RETRY_INTERVAL;
use crate::lispm::Effect;
use crate::wire::channel_of;

/// A later version than this one.
const LATER: u8 = VERSION + 1;

/// `frame` as an end of the version `version` would send it.
fn in_version(version: u8, mut frame: Vec<u8>) -> Vec<u8> {
    frame[0] = version;
    frame
}

/// LispM of another version turning away the device's `frame`, which it
/// cannot read: a refusal on the frame's channel, in its own version.
fn answered_by(version: u8, frame: &[u8]) -> Vec<u8> {
    let refusal = Frame::Refuse {
        channel: channel_of(frame).unwrap(),
        reason: Refusal::OtherVersion,
    };
    in_version(version, refusal.encode())
}

fn other_version(effects: &[device::Effect]) -> Option<u8> {
    effects.iter().find_map(|effect| match effect {
        device::Effect::Failed(Failure::OtherVersion(version)) => Some(*version),
        _ => None,
    })
}

#[test]
fn lispm_answers_a_device_of_another_version_in_its_own() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let token = lispm.offer();
    let (init, _) = phone.pair(&token);
    let later = in_version(LATER, init);

    let effects = lispm.receive(&phone.peer, later.clone());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::OtherVersion { from, version } if *from == phone.peer && *version == LATER
    )));
    let answer = single(frames_to(&effects, &phone.peer));
    assert_eq!(answer.first(), Some(&VERSION));
    assert_eq!(
        Frame::decode(&answer),
        Ok(Frame::Refuse {
            channel: channel_of(&later).unwrap(),
            reason: Refusal::OtherVersion,
        })
    );
    // The frame claims nothing: the offer stays for a device that can.
    assert_eq!(question(&effects), None);
    assert!(lispm.state().offer().is_some());
}

#[test]
fn lispm_names_a_version_it_cannot_answer() {
    let mut lispm = Lispm::new(Vec::new());
    let stranger = peer("did:rings:stranger");
    let effects = lispm.receive(&stranger, vec![LATER]);
    assert!(frames_to(&effects, &stranger).is_empty());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::OtherVersion { version, .. } if *version == LATER
    )));
}

#[test]
fn a_pairing_device_stops_at_lispm_of_another_version() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let token = lispm.offer();
    let (init, _) = phone.pair(&token);

    let effects = phone.receive(&lispm.peer(), answered_by(LATER, &init));
    assert_eq!(other_version(&effects), Some(LATER));
    // The pairing request is not sent again.
    phone.now = START.after(RETRY_INTERVAL * 3);
    assert!(transmitted(&phone.step(device::Input::Tick).unwrap()).is_empty());
    assert_eq!(phone.state().pairing(), None);
}

#[test]
fn an_opening_device_stops_at_lispm_of_another_version_and_keeps_its_pairing() {
    let mut first = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    pair(&mut first, &mut phone);
    let pairing = phone.state().pairing().cloned();
    let mut phone = Phone::new("did:rings:phone", 7, pairing.clone());

    let init = phone.connect();
    let effects = phone.receive(&first.peer(), answered_by(LATER, &init));
    assert_eq!(other_version(&effects), Some(LATER));
    phone.now = START.after(RETRY_INTERVAL * 3);
    assert!(transmitted(&phone.step(device::Input::Tick).unwrap()).is_empty());
    // Updating either end fixes it, so the device stays paired.
    assert_eq!(phone.state().pairing().cloned(), pairing);
}

#[test]
fn an_open_session_ends_at_a_frame_of_another_version() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (_, channel) = pair(&mut lispm, &mut phone);
    // LispM restarted in another version; what it sends is not read.
    let record = in_version(LATER, lispm.send(&phone.peer, channel, "state"));

    let effects = phone.receive(&lispm.peer(), record);
    assert!(matches!(
        effects.first(),
        Some(device::Effect::Closed(device::CloseReason::ByPeer))
    ));
    assert_eq!(other_version(&effects), Some(LATER));
    assert!(!phone.state().is_open());
}
