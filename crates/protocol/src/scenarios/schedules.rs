// lispm-remote: schedules, not traces.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests over every order of a set of concurrent events, and an
//! induction over the number of devices.
//!
//! - Pairing: the events of one offer (the device's claim, the network
//!   echoing it, a second device with the same QR code, the owner's
//!   approval, expiry) in all 120 orders.  Safety and progress are one
//!   characterization: the device pairs iff its claim arrives first, the
//!   owner approves before the second device and before expiry.
//! - Records: every delivery order of a channel's records, each repeated.
//! - Devices: the reduction is the projection of a run onto one device's
//!   frames.  Devices share no state but the responder's map, keyed by
//!   peer, so the projection of `n + 1` devices onto the first `n` equals
//!   the run of `n` devices: checked for `n = 1..4`, with `n = 1` checked
//!   against the expected transcript.

use std::collections::BTreeMap;

use super::*;
use crate::lispm::Input;

fn permutations<T: Copy>(items: &[T]) -> Vec<Vec<T>> {
    if items.is_empty() {
        return vec![Vec::new()];
    }
    let mut all = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let mut rest = items.to_vec();
        rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, *item);
            all.push(tail);
        }
    }
    all
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// The device's claim arrives.
    Claim,
    /// The network delivers the same claim again.
    Echo,
    /// A second device, which photographed the QR code, claims.
    Thief,
    /// The owner approves the fingerprint the device shows.
    Approve,
    /// The offer's time runs out.
    Expire,
}

const EVENTS: [Event; 5] = [
    Event::Claim,
    Event::Echo,
    Event::Thief,
    Event::Approve,
    Event::Expire,
];

/// The devices paired when the events of one offer happen in `schedule`.
fn run(schedule: &[Event]) -> Vec<Device> {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let mut thief = Phone::new("did:rings:thief", 8, None);
    let (claim, fingerprint) = phone.pair(&token);
    let (stolen, _) = thief.pair(&token);
    let mut stored = Vec::new();
    for event in schedule {
        let effects = match event {
            Event::Claim | Event::Echo => lispm.receive(&phone.peer, claim.clone()),
            Event::Thief => lispm.receive(&thief.peer, stolen.clone()),
            Event::Approve => lispm
                .step(Input::Approve { fingerprint })
                .unwrap_or_default(),
            Event::Expire => lispm.tick(Millis::new(START.get() + OFFER_SPAN)),
        };
        stored.extend(paired(&effects));
    }
    stored
}

#[test]
fn across_all_orders_the_device_pairs_iff_approved_first_and_uncontested() {
    let schedules = permutations(&EVENTS);
    assert_eq!(schedules.len(), 120);
    for schedule in schedules {
        let at = |event| schedule.iter().position(|e| *e == event).unwrap();
        let arrived = at(Event::Claim).min(at(Event::Echo));
        let approved = at(Event::Approve);
        let pairs =
            arrived < approved && approved < at(Event::Thief) && approved < at(Event::Expire);

        let stored = run(&schedule);
        assert!(stored.len() <= 1, "two pairings in {schedule:?}");
        assert_eq!(!stored.is_empty(), pairs, "{schedule:?}");
        for device in stored {
            assert_eq!(device.peer(), &peer("did:rings:phone"), "{schedule:?}");
        }
    }
}

#[test]
fn records_arrive_exactly_once_in_every_order_with_every_record_repeated() {
    let texts = ["m0", "m1", "m2", "m3"];
    for order in permutations(&[0_usize, 1, 2, 3]) {
        let mut lispm = Lispm::new(Vec::new());
        let mut phone = Phone::new("did:rings:phone", 7, None);
        pair(&mut lispm, &mut phone);
        let frames: Vec<Vec<u8>> = texts.iter().map(|text| phone.send(text)).collect();

        let mut got = Vec::new();
        for &i in order.iter().chain(order.iter().rev()) {
            got.extend(delivered(&lispm.receive(&phone.peer, frames[i].clone())));
        }
        let got: Vec<&str> = got.iter().map(|(_, text)| text.as_str()).collect();
        let expected: Vec<&str> = order.iter().map(|&i| texts[i]).collect();
        assert_eq!(got, expected, "{order:?}");
    }
}

#[test]
fn a_lost_record_holds_back_no_other() {
    for lost in 0..4 {
        let mut lispm = Lispm::new(Vec::new());
        let mut phone = Phone::new("did:rings:phone", 7, None);
        pair(&mut lispm, &mut phone);
        let frames: Vec<Vec<u8>> = (0..4).map(|i| phone.send(&format!("m{i}"))).collect();
        let got: Vec<String> = frames
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != lost)
            .flat_map(|(_, frame)| delivered(&lispm.receive(&phone.peer, frame)))
            .map(|(_, text)| text)
            .collect();
        let expected: Vec<String> = (0..4)
            .filter(|i| *i != lost)
            .map(|i| format!("m{i}"))
            .collect();
        assert_eq!(got, expected);
    }
}

/// Each device's view of a run: what LispM received from it, and what it
/// received from LispM.
type Transcripts = BTreeMap<PeerId, (Vec<String>, Vec<String>)>;

/// `n` devices pair in turn, then talk in interleaved rounds.
fn conversation(n: usize) -> Transcripts {
    let mut lispm = Lispm::new(Vec::new());
    let mut phones: Vec<Phone> = (0..n)
        .map(|i| {
            Phone::new(
                &format!("did:rings:p{i}"),
                10 + u8::try_from(i).unwrap(),
                None,
            )
        })
        .collect();
    let channels: Vec<ChannelId> = phones
        .iter_mut()
        .map(|phone| pair(&mut lispm, phone).1)
        .collect();
    let mut transcripts = Transcripts::new();
    for round in 0..3 {
        for (phone, channel) in phones.iter_mut().zip(&channels) {
            let up = phone.send(&format!("{}-up{round}", phone.peer));
            for (from, text) in delivered(&lispm.receive(&phone.peer, up)) {
                transcripts.entry(from).or_default().0.push(text);
            }
            let down = lispm.send(
                &phone.peer,
                *channel,
                &format!("{}-down{round}", phone.peer),
            );
            let texts = phone_delivered(&phone.receive(&lispm.peer(), down));
            transcripts
                .entry(phone.peer.clone())
                .or_default()
                .1
                .extend(texts);
        }
    }
    transcripts
}

#[test]
fn adding_a_device_leaves_every_other_conversation_as_it_was() {
    // Base: one device's transcript is exactly what it and LispM sent.
    let one = conversation(1);
    let p0 = peer("did:rings:p0");
    let ups: Vec<String> = (0..3).map(|r| format!("did:rings:p0-up{r}")).collect();
    let downs: Vec<String> = (0..3).map(|r| format!("did:rings:p0-down{r}")).collect();
    assert_eq!(one.get(&p0), Some(&(ups, downs)));

    // Step: the run of n + 1 devices projects onto the run of n.
    for n in 1..=4 {
        let small = conversation(n);
        let large = conversation(n + 1);
        assert_eq!(large.len(), n + 1);
        for (device, transcript) in &small {
            assert_eq!(large.get(device), Some(transcript), "n = {n}");
        }
    }
}
