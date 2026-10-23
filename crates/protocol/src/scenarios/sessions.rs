// lispm-remote: session scenarios.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::OpenError;
use crate::Refusal;
use crate::device::CloseReason as DeviceClose;
use crate::device::CommandError as DeviceError;
use crate::device::Discard as DeviceDiscard;
use crate::device::Failure;
use crate::device::OPEN_TIMEOUT;
use crate::device::RETRY_INTERVAL;
use crate::lispm::CloseReason;
use crate::lispm::Discard;
use crate::lispm::Effect;
use crate::lispm::Input;
use crate::lispm::MAX_CHANNELS_PER_DEVICE;
use crate::noise::HandshakeError;

/// A responder and a phone paired with it, both restarted from their
/// stored records.
fn restarted() -> (Lispm, Phone) {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (record, _) = pair(&mut lispm, &mut phone);
    let pairing = phone.state().pairing().cloned();
    (
        Lispm::new(vec![record]),
        Phone::new("did:rings:phone", 7, pairing),
    )
}

#[test]
fn a_paired_device_reconnects_after_both_ends_restart() {
    let (mut lispm, mut phone) = restarted();
    let channel = connect(&mut lispm, &mut phone);
    assert_eq!(
        lispm.state().channels(&phone.peer).collect::<Vec<_>>(),
        vec![channel]
    );

    let up = phone.send("agents?");
    assert_eq!(
        delivered(&lispm.receive(&phone.peer, up)),
        vec![(phone.peer.clone(), "agents?".to_owned())]
    );
    let down = lispm.send(&phone.peer, channel, "coder: idle");
    assert_eq!(
        phone_delivered(&phone.receive(&lispm.peer(), down)),
        vec!["coder: idle".to_owned()]
    );
}

#[test]
fn an_unpaired_device_is_told_and_forgets_its_pairing() {
    let (_, mut phone) = restarted();
    let mut amnesiac = Lispm::new(Vec::new());
    let init = phone.connect();
    let effects = amnesiac.receive(&phone.peer, init);
    assert_eq!(discarded(&effects), Some(Discard::NotPaired));
    let frame = single(frames_to(&effects, &phone.peer));
    assert_eq!(refusal(&frame), Some(Refusal::NotPaired));

    let told = phone.receive(&amnesiac.peer(), frame);
    assert!(told.iter().any(|e| matches!(e, device::Effect::Unpaired)));
    assert!(phone.state().pairing().is_none());
    assert_eq!(
        phone
            .step(device::Input::Connect {
                entropy: phone_seed()
            })
            .err(),
        Some(DeviceError::NotPaired)
    );
}

fn phone_seed() -> Entropy {
    Entropy::new([0x42; ENTROPY_LEN])
}

#[test]
fn a_forgotten_device_is_closed_and_cannot_reconnect() {
    let (mut lispm, mut phone) = restarted();
    let channel = connect(&mut lispm, &mut phone);

    let effects = lispm
        .step(Input::Forget {
            peer: phone.peer.clone(),
        })
        .unwrap();
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Closed { channel: c, why: CloseReason::Forgotten, .. } if *c == channel
    )));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Forgotten(p) if *p == phone.peer))
    );
    let close = single(frames_to(&effects, &phone.peer));
    let told = phone.receive(&lispm.peer(), close);
    assert!(
        told.iter()
            .any(|e| matches!(e, device::Effect::Closed(DeviceClose::ByPeer)))
    );

    let init = phone.connect();
    let effects = lispm.receive(&phone.peer, init);
    assert_eq!(discarded(&effects), Some(Discard::NotPaired));
}

#[test]
fn a_tampered_record_is_discarded_and_the_real_one_still_arrives() {
    let (mut lispm, mut phone) = restarted();
    connect(&mut lispm, &mut phone);
    let frame = phone.send("approve 1");
    let mut tampered = frame.clone();
    if let Some(last) = tampered.last_mut() {
        *last ^= 1;
    }
    assert_eq!(
        discarded(&lispm.receive(&phone.peer, tampered)),
        Some(Discard::Record(OpenError::Forged))
    );
    assert_eq!(
        delivered(&lispm.receive(&phone.peer, frame.clone())),
        vec![(phone.peer.clone(), "approve 1".to_owned())]
    );
    assert_eq!(
        discarded(&lispm.receive(&phone.peer, frame)),
        Some(Discard::Record(OpenError::Replayed))
    );
}

#[test]
fn a_record_reaches_only_its_senders_channels() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let mut other = Phone::new("did:rings:other", 8, None);
    pair(&mut lispm, &mut phone);
    pair(&mut lispm, &mut other);

    let frame = phone.send("mine");
    assert_eq!(
        discarded(&lispm.receive(&peer("did:rings:stranger"), frame.clone())),
        Some(Discard::NotPaired)
    );
    assert_eq!(
        discarded(&lispm.receive(&other.peer, frame.clone())),
        Some(Discard::UnknownChannel)
    );
    assert_eq!(delivered(&lispm.receive(&phone.peer, frame)).len(), 1);
}

#[test]
fn an_open_relayed_under_another_paired_address_fails_its_handshake() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let mut other = Phone::new("did:rings:other", 8, None);
    pair(&mut lispm, &mut phone);
    pair(&mut lispm, &mut other);

    // Another tab of the first phone opens a session; the frame is
    // delivered as if the second phone had sent it.
    let init = phone.reloaded().connect();
    let effects = lispm.receive(&other.peer, init);
    assert_eq!(
        discarded(&effects),
        Some(Discard::Handshake(HandshakeError::Rejected))
    );
    assert!(opened(&effects).is_empty());
}

#[test]
fn a_repeated_open_gets_the_first_answer_again() {
    let (mut lispm, mut phone) = restarted();
    let init = phone.connect();
    let first = lispm.receive(&phone.peer, init.clone());
    let channel = single(opened(&first));
    let accept = single(frames_to(&first, &phone.peer));

    // The answer is lost; at its next tick the phone sends its frame again.
    phone.now = START.after(RETRY_INTERVAL);
    let again = single(transmitted(&phone.step(device::Input::Tick).unwrap()));
    assert_eq!(again, init);
    let second = lispm.receive(&phone.peer, again);
    assert!(opened(&second).is_empty());
    assert_eq!(single(frames_to(&second, &phone.peer)), accept);

    // Both ends hold the keys of one handshake.
    let effects = phone.receive(&lispm.peer(), accept);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Opened(id) if *id == channel))
    );
    let up = phone.send("ok");
    assert_eq!(delivered(&lispm.receive(&phone.peer, up)).len(), 1);
    let down = lispm.send(&phone.peer, channel, "still here");
    assert_eq!(
        phone_delivered(&phone.receive(&lispm.peer(), down)).len(),
        1
    );

    // Its record shows the phone has the keys: the answer is not kept.
    assert_eq!(
        discarded(&lispm.receive(&phone.peer, init)),
        Some(Discard::Duplicate)
    );
}

#[test]
fn a_lost_open_frame_is_sent_again_at_the_retry_interval() {
    let (mut lispm, mut phone) = restarted();
    let init = phone.connect();
    phone.now = Millis::new(START.after(RETRY_INTERVAL).get() - 1);
    assert!(phone.step(device::Input::Tick).unwrap().is_empty());
    phone.now = START.after(RETRY_INTERVAL);
    let again = single(transmitted(&phone.step(device::Input::Tick).unwrap()));
    assert_eq!(again, init);
    // The first one never arrived; the second opens the channel.
    let answered = lispm.receive(&phone.peer, again);
    let channel = single(opened(&answered));
    let accept = single(frames_to(&answered, &phone.peer));
    let effects = phone.receive(&lispm.peer(), accept);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Opened(id) if *id == channel))
    );
}

#[test]
fn another_frame_on_an_open_channel_is_discarded() {
    let (mut lispm, mut phone) = restarted();
    let init = phone.connect();
    lispm.receive(&phone.peer, init.clone());
    let mut other = init;
    if let Some(last) = other.last_mut() {
        *last ^= 1;
    }
    assert_eq!(
        discarded(&lispm.receive(&phone.peer, other)),
        Some(Discard::Duplicate)
    );
}

#[test]
fn the_oldest_channel_is_evicted_past_the_cap() {
    let (mut lispm, phone) = restarted();
    let mut tabs: Vec<Phone> = Vec::new();
    let mut channels = Vec::new();
    for i in 0..=MAX_CHANNELS_PER_DEVICE {
        let mut tab = phone.reloaded();
        tab.seeds = Seeds(10_000 * (u64::try_from(i).unwrap() + 1));
        let init = tab.connect();
        let effects = lispm.receive(&tab.peer, init);
        channels.push(single(opened(&effects)));
        if i == MAX_CHANNELS_PER_DEVICE {
            assert!(effects.iter().any(|e| matches!(
                e,
                Effect::Closed { channel: c, why: CloseReason::Evicted, .. } if *c == channels[0]
            )));
        }
        tabs.push(tab);
    }
    let open: Vec<_> = lispm.state().channels(&phone.peer).collect();
    assert_eq!(open.len(), MAX_CHANNELS_PER_DEVICE);
    assert!(!open.contains(&channels[0]));
}

#[test]
fn closing_tells_the_other_end() {
    let (mut lispm, mut phone) = restarted();
    connect(&mut lispm, &mut phone);
    let effects = phone.step(device::Input::Close).unwrap();
    let close = single(transmitted(&effects));
    let effects = lispm.receive(&phone.peer, close);
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Closed {
            why: CloseReason::ByPeer,
            ..
        }
    )));
    assert_eq!(lispm.state().channels(&phone.peer).count(), 0);

    let channel = connect(&mut lispm, &mut phone);
    let effects = lispm
        .step(Input::Close {
            peer: phone.peer.clone(),
            channel,
        })
        .unwrap();
    let close = single(frames_to(&effects, &phone.peer));
    let told = phone.receive(&lispm.peer(), close);
    assert!(
        told.iter()
            .any(|e| matches!(e, device::Effect::Closed(DeviceClose::ByPeer)))
    );
    assert!(!phone.state().is_open());
}

#[test]
fn opening_times_out_without_an_answer() {
    let (_, mut phone) = restarted();
    phone.connect();
    phone.now = Millis::new(START.get() + 1);
    assert!(phone.step(device::Input::Tick).unwrap().is_empty());
    phone.now = START.after(OPEN_TIMEOUT);
    let effects = phone.step(device::Input::Tick).unwrap();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Failed(Failure::TimedOut)))
    );
    assert!(phone.state().pairing().is_some());
}

#[test]
fn the_device_discards_frames_from_strangers() {
    let (mut lispm, mut phone) = restarted();
    connect(&mut lispm, &mut phone);
    let effects = phone.receive(&peer("did:rings:stranger"), vec![1, 2, 3]);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Discarded(DeviceDiscard::Stranger)))
    );
}

#[test]
fn device_commands_out_of_phase_are_refused() {
    let mut lonely = Phone::new("did:rings:lonely", 3, None);
    assert_eq!(
        lonely.step(device::Input::Send(payload("x"))).err(),
        Some(DeviceError::NotOpen)
    );
    assert_eq!(
        lonely
            .step(device::Input::Connect {
                entropy: phone_seed()
            })
            .err(),
        Some(DeviceError::NotPaired)
    );
    assert_eq!(
        lonely.step(device::Input::Close).err(),
        Some(DeviceError::NotOpen)
    );

    let (_, mut phone) = restarted();
    phone.connect();
    assert_eq!(
        phone
            .step(device::Input::Connect {
                entropy: phone_seed()
            })
            .err(),
        Some(DeviceError::Busy)
    );
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let offer = Offer::from_token(&token).unwrap();
    assert_eq!(
        phone
            .step(device::Input::Pair {
                offer,
                name: name("Busy"),
                entropy: phone_seed(),
            })
            .err(),
        Some(DeviceError::Busy)
    );
}

#[test]
fn lispm_commands_on_missing_channels_are_refused() {
    let (mut lispm, phone) = restarted();
    let channel = ChannelId::from_bytes([0; crate::CHANNEL_LEN]);
    assert_eq!(
        lispm
            .step(Input::Send {
                peer: phone.peer.clone(),
                channel,
                payload: payload("x"),
            })
            .err(),
        Some(crate::lispm::CommandError::NoSuchChannel)
    );
    assert_eq!(
        lispm
            .step(Input::Close {
                peer: phone.peer.clone(),
                channel,
            })
            .err(),
        Some(crate::lispm::CommandError::NoSuchChannel)
    );
    assert_eq!(
        lispm
            .step(Input::Forget {
                peer: peer("did:rings:nobody"),
            })
            .err(),
        Some(crate::lispm::CommandError::NoSuchDevice)
    );
}
