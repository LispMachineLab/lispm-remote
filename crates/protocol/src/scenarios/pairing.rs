// lispm-remote: pairing scenarios.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;
use crate::Refusal;
use crate::lispm::CloseReason;
use crate::lispm::CommandError;
use crate::lispm::Discard;
use crate::lispm::Input;
use crate::lispm::MAX_DEVICES;
use crate::lispm::OfferEnd;
use crate::noise::HandshakeError;

#[test]
fn an_approved_claim_pairs_the_device_and_opens_its_channel() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (record, channel) = pair(&mut lispm, &mut phone);

    assert_eq!(record.peer(), &phone.peer);
    assert_eq!(record.key(), phone.state().identity().public_key());
    assert_eq!(record.name().as_str(), "Test Phone");
    assert_eq!(lispm.state().devices().collect::<Vec<_>>(), vec![&record]);
    assert!(lispm.state().offer().is_none());
    let pairing = phone.state().pairing().unwrap();
    assert_eq!(pairing.responder(), &lispm.peer());
    assert_eq!(pairing.key(), lispm.state().identity().public_key());

    let up = phone.send("state?");
    assert_eq!(
        delivered(&lispm.receive(&phone.peer, up)),
        vec![(phone.peer.clone(), "state?".to_owned())]
    );
    let down = lispm.send(&phone.peer, channel, "two agents running");
    assert_eq!(
        phone_delivered(&phone.receive(&lispm.peer(), down)),
        vec!["two agents running".to_owned()]
    );
}

#[test]
fn a_lost_pairing_answer_is_sent_again_after_approval() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, fingerprint) = phone.pair(&token);
    lispm.receive(&phone.peer, init.clone());
    let approved = lispm.step(Input::Approve { fingerprint }).unwrap();
    let channel = single(opened(&approved));
    let accept = single(frames_to(&approved, &phone.peer));

    // The answer is lost; the offer is used up, yet the phone's frame,
    // sent again, gets the same answer and no refusal.
    phone.now = START.after(crate::device::RETRY_INTERVAL);
    let again = single(transmitted(&phone.step(device::Input::Tick).unwrap()));
    assert_eq!(again, init);
    let repeated = lispm.receive(&phone.peer, again);
    assert!(paired(&repeated).is_empty());
    assert_eq!(single(frames_to(&repeated, &phone.peer)), accept);

    let effects = phone.receive(&lispm.peer(), accept);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Paired(_)))
    );
    let up = phone.send("hello");
    assert_eq!(delivered(&lispm.receive(&phone.peer, up)).len(), 1);
    let down = lispm.send(&phone.peer, channel, "welcome");
    assert_eq!(
        phone_delivered(&phone.receive(&lispm.peer(), down)).len(),
        1
    );
}

#[test]
fn a_claim_without_the_offer_key_is_discarded_and_the_offer_stays() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    // Everything the QR code says except its pre-shared key.
    let mut bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
    for byte in &mut bytes[1 + 16 + 32..1 + 16 + 32 + 32] {
        *byte ^= 0x5a;
    }
    let forged = URL_SAFE_NO_PAD.encode(&bytes);

    let mut guesser = Phone::new("did:rings:guesser", 9, None);
    let (init, _) = guesser.pair(&forged);
    let effects = lispm.receive(&guesser.peer, init);
    assert_eq!(
        discarded(&effects),
        Some(Discard::Handshake(HandshakeError::Rejected))
    );
    assert!(question(&effects).is_none());
    assert!(lispm.state().offer().is_some());

    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, fingerprint) = phone.pair(&token);
    assert_eq!(
        question(&lispm.receive(&phone.peer, init)),
        Some(fingerprint)
    );
}

#[test]
fn a_second_device_with_the_key_contests_the_offer_and_nobody_pairs() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let mut thief = Phone::new("did:rings:thief", 8, None);
    let (first, fingerprint) = phone.pair(&token);
    let (second, _) = thief.pair(&token);

    assert!(question(&lispm.receive(&phone.peer, first)).is_some());
    let effects = lispm.receive(&thief.peer, second);
    assert_eq!(offer_end(&effects), Some(OfferEnd::Contested));
    for device in [&phone.peer, &thief.peer] {
        let frame = single(frames_to(&effects, device));
        assert_eq!(refusal(&frame), Some(Refusal::Contested));
    }
    assert!(paired(&effects).is_empty());
    assert_eq!(
        lispm.step(Input::Approve { fingerprint }).err(),
        Some(CommandError::NoClaim)
    );
    assert_eq!(lispm.state().devices().count(), 0);
}

#[test]
fn the_same_device_claiming_again_asks_again_and_only_the_latest_answer_pairs() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (first, old) = phone.pair(&token);
    assert_eq!(question(&lispm.receive(&phone.peer, first)), Some(old));

    // The page reloads with its key and claims again.
    let mut reloaded = phone.reloaded();
    let (second, new) = reloaded.pair(&token);
    assert_ne!(old, new);
    assert_eq!(question(&lispm.receive(&reloaded.peer, second)), Some(new));

    assert_eq!(
        lispm.step(Input::Approve { fingerprint: old }).err(),
        Some(CommandError::StaleApproval)
    );
    let approved = lispm.step(Input::Approve { fingerprint: new }).unwrap();
    assert_eq!(paired(&approved).len(), 1);
    let accept = single(frames_to(&approved, &reloaded.peer));

    // The answer belongs to the second handshake only.
    let stale = phone.receive(&lispm.peer(), accept.clone());
    assert!(stale.iter().any(|e| matches!(e, device::Effect::Failed(_))));
    let fresh = reloaded.receive(&lispm.peer(), accept);
    assert!(fresh.iter().any(|e| matches!(e, device::Effect::Paired(_))));
}

#[test]
fn a_refused_command_leaves_the_claim_as_it_was() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, fingerprint) = phone.pair(&token);
    lispm.receive(&phone.peer, init);

    let wrong = Fingerprint::of_handshake(b"something else");
    assert_eq!(
        lispm.step(Input::Approve { fingerprint: wrong }).err(),
        Some(CommandError::StaleApproval)
    );
    let approved = lispm.step(Input::Approve { fingerprint }).unwrap();
    assert_eq!(paired(&approved).len(), 1);
}

#[test]
fn an_expired_offer_turns_claims_away() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, _) = phone.pair(&token);

    lispm.now = Millis::new(START.get() + OFFER_SPAN);
    let effects = lispm.receive(&phone.peer, init);
    assert_eq!(offer_end(&effects), Some(OfferEnd::Expired));
    assert_eq!(discarded(&effects), Some(Discard::Expired));
    let frame = single(frames_to(&effects, &phone.peer));
    assert_eq!(refusal(&frame), Some(Refusal::OfferUnavailable));
    let told = phone.receive(&lispm.peer(), frame);
    assert!(told.iter().any(|e| matches!(
        e,
        device::Effect::Failed(device::Failure::Refused(Refusal::OfferUnavailable))
    )));
}

#[test]
fn a_claim_waiting_past_expiry_cannot_be_approved_and_ends_at_the_next_tick() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, fingerprint) = phone.pair(&token);
    lispm.receive(&phone.peer, init);

    let end = Millis::new(START.get() + OFFER_SPAN);
    assert!(lispm.tick(Millis::new(end.get() - 1)).is_empty());
    lispm.now = end;
    assert_eq!(
        lispm.step(Input::Approve { fingerprint }).err(),
        Some(CommandError::Expired)
    );
    let effects = lispm.tick(end);
    assert_eq!(offer_end(&effects), Some(OfferEnd::Expired));
    let frame = single(frames_to(&effects, &phone.peer));
    assert_eq!(refusal(&frame), Some(Refusal::OfferUnavailable));
    assert!(lispm.state().offer().is_none());
}

#[test]
fn the_device_times_out_when_the_offer_expires() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    phone.pair(&token);
    phone.now = Millis::new(START.get() + OFFER_SPAN);
    let effects = phone.step(device::Input::Tick).unwrap();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, device::Effect::Failed(device::Failure::TimedOut)))
    );
}

#[test]
fn the_owner_refusing_tells_the_device() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, _) = phone.pair(&token);
    lispm.receive(&phone.peer, init);

    let effects = lispm.step(Input::Refuse).unwrap();
    assert_eq!(offer_end(&effects), Some(OfferEnd::Refused));
    let frame = single(frames_to(&effects, &phone.peer));
    let told = phone.receive(&lispm.peer(), frame);
    assert!(told.iter().any(|e| matches!(
        e,
        device::Effect::Failed(device::Failure::Refused(Refusal::OwnerRefused))
    )));
    assert!(phone.state().pairing().is_none());
}

#[test]
fn a_claim_relayed_under_another_address_fails_its_handshake() {
    let mut lispm = Lispm::new(Vec::new());
    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, _) = phone.pair(&token);

    // A relay that read the frame sends it on as its own.
    let effects = lispm.receive(&peer("did:rings:relay"), init);
    assert_eq!(
        discarded(&effects),
        Some(Discard::Handshake(HandshakeError::Rejected))
    );
    assert!(question(&effects).is_none());
}

#[test]
fn a_new_offer_replaces_the_old_one_and_its_claims() {
    let mut lispm = Lispm::new(Vec::new());
    let old = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, _) = phone.pair(&old);
    lispm.receive(&phone.peer, init);

    let expires = Millis::new(START.get() + OFFER_SPAN);
    let entropy = lispm.seeds.draw();
    let effects = lispm.step(Input::Offer { expires, entropy }).unwrap();
    assert_eq!(offer_end(&effects), Some(OfferEnd::Replaced));
    let frame = single(frames_to(&effects, &phone.peer));
    assert_eq!(refusal(&frame), Some(Refusal::OfferUnavailable));

    let mut late = Phone::new("did:rings:late", 8, None);
    let (init, _) = late.pair(&old);
    let effects = lispm.receive(&late.peer, init);
    assert_eq!(discarded(&effects), Some(Discard::NoOffer));
    assert!(lispm.state().offer().is_some());
}

#[test]
fn commands_the_offer_does_not_permit_are_refused() {
    let mut lispm = Lispm::new(Vec::new());
    assert_eq!(
        lispm.step(Input::Withdraw).err(),
        Some(CommandError::NoOffer)
    );
    assert_eq!(lispm.step(Input::Refuse).err(), Some(CommandError::NoClaim));
    let entropy = lispm.seeds.draw();
    assert_eq!(
        lispm
            .step(Input::Offer {
                expires: START,
                entropy
            })
            .err(),
        Some(CommandError::InvalidExpiry)
    );
    lispm.offer();
    assert_eq!(lispm.step(Input::Refuse).err(), Some(CommandError::NoClaim));
    let effects = lispm.step(Input::Withdraw).unwrap();
    assert_eq!(offer_end(&effects), Some(OfferEnd::Withdrawn));
}

#[test]
fn pairing_again_replaces_the_record_and_closes_the_old_channels() {
    let mut lispm = Lispm::new(Vec::new());
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (old, channel) = pair(&mut lispm, &mut phone);

    // The same address with a new key, as after the page lost its storage.
    let mut fresh = Phone::new("did:rings:phone", 70, None);
    let token = lispm.offer();
    let (init, fingerprint) = fresh.pair(&token);
    lispm.receive(&fresh.peer, init);
    let effects = lispm.step(Input::Approve { fingerprint }).unwrap();
    assert!(effects.iter().any(|e| matches!(
        e,
        crate::lispm::Effect::Closed { channel: c, why: CloseReason::Repaired, .. } if *c == channel
    )));
    let record = single(paired(&effects));
    assert_ne!(record.key(), old.key());
    assert_eq!(lispm.state().devices().collect::<Vec<_>>(), vec![&record]);
}

#[test]
fn the_device_cap_holds_for_new_addresses_only() {
    let records = (0..MAX_DEVICES)
        .map(|i| {
            Device::new(
                peer(&format!("did:rings:old{i}")),
                crate::PublicKey::from_bytes([1; KEY_LEN]),
                name("Old"),
                START,
            )
        })
        .collect();
    let mut lispm = Lispm::new(records);

    let token = lispm.offer();
    let mut phone = Phone::new("did:rings:phone", 7, None);
    let (init, fingerprint) = phone.pair(&token);
    lispm.receive(&phone.peer, init);
    assert_eq!(
        lispm.step(Input::Approve { fingerprint }).err(),
        Some(CommandError::TooManyDevices)
    );
    lispm.step(Input::Withdraw).unwrap();

    let token = lispm.offer();
    let mut known = Phone::new("did:rings:old3", 9, None);
    let (init, fingerprint) = known.pair(&token);
    lispm.receive(&known.peer, init);
    assert!(lispm.step(Input::Approve { fingerprint }).is_ok());
    assert_eq!(lispm.state().devices().count(), MAX_DEVICES);
}
