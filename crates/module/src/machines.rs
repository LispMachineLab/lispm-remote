// lispm-remote: the pairing and session machines, as Lisp sees them.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! LispM's `Responder` and a device's `Initiator` from
//! `lispm-remote-protocol`, each in a `user-ptr`.  Every function steps the
//! machine once, with this computer's clock and fresh randomness from the
//! operating system, and returns the effects as a list for Lisp to carry
//! out; frames to send are among them, so the network stays with the node.
//!
//! ```text
//!   (transmit TO FRAME)            send FRAME to the DID TO
//!   (show-offer TOKEN)             show TOKEN as a QR code
//!   (ask-owner NAME FINGERPRINT)   ask the owner about the device NAME
//!   (offer-ended REASON)           the offer ended: used, expired, refused, ...
//!   (paired PEER KEY NAME AT)      store the device
//!   (forgotten PEER)               delete the stored device
//!   (opened PEER CHANNEL)          a channel opened
//!   (delivered PEER CHANNEL BYTES) a channel delivered a payload
//!   (closed PEER CHANNEL REASON)   a channel closed
//!   (discarded FROM WHY)           a frame was discarded
//! ```
//!
//! A device's effects are the same without PEER, plus `(show-fingerprint
//! FINGERPRINT)`, `(unpaired)` and `(failed REASON [DETAIL])`.  Keys and
//! channels travel as hexadecimal text, fingerprints as `4F2A-91C0`,
//! frames and payloads as unibyte strings, moments as milliseconds since
//! the epoch.  A command the machine refuses signals an error and leaves
//! the machine as it was.

use std::fmt::Debug;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use emacs::Env;
use emacs::IntoLisp;
use emacs::Result;
use emacs::Value;
use emacs::Vector;
use emacs::defun;
use lispm_remote_protocol::ChannelId;
use lispm_remote_protocol::Device;
use lispm_remote_protocol::DeviceName;
use lispm_remote_protocol::ENTROPY_LEN;
use lispm_remote_protocol::Entropy;
use lispm_remote_protocol::Fingerprint;
use lispm_remote_protocol::Identity;
use lispm_remote_protocol::Initiator;
use lispm_remote_protocol::KEY_LEN;
use lispm_remote_protocol::Millis;
use lispm_remote_protocol::Offer;
use lispm_remote_protocol::Pairing;
use lispm_remote_protocol::Payload;
use lispm_remote_protocol::PeerId;
use lispm_remote_protocol::PublicKey;
use lispm_remote_protocol::Responder;
use lispm_remote_protocol::StaticSecret;
use lispm_remote_protocol::device;
use lispm_remote_protocol::lispm;

/// LispM's machine, absent only while it steps.
struct LispmMachine(Option<Responder>);

/// A device's machine, absent only while it steps.
struct DeviceMachine(Option<Initiator>);

// ----------------------------------------------------------------------
// Values from Lisp

fn now() -> Millis {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    Millis::new(u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

fn entropy() -> Result<Entropy> {
    let mut bytes = [0_u8; ENTROPY_LEN];
    getrandom::fill(&mut bytes).map_err(|error| emacs::Error::msg(error.to_string()))?;
    Ok(Entropy::new(bytes))
}

fn peer(text: &str) -> Result<PeerId> {
    Ok(PeerId::new(text.as_bytes())?)
}

fn bytes<const N: usize>(text: &str, what: &str) -> Result<[u8; N]> {
    let decoded = hex::decode(text)?;
    <[u8; N]>::try_from(decoded.as_slice())
        .map_err(|_| emacs::Error::msg(format!("{what} is {N} bytes in hexadecimal")))
}

fn channel(text: &str) -> Result<ChannelId> {
    bytes(text, "a channel").map(ChannelId::from_bytes)
}

fn key(text: &str) -> Result<PublicKey> {
    bytes(text, "a key").map(PublicKey::from_bytes)
}

fn identity(secret: &str, did: &str) -> Result<Identity> {
    let secret = StaticSecret::from_bytes(bytes::<KEY_LEN>(secret, "a static secret")?);
    Ok(Identity::new(peer(did)?, secret)?)
}

/// The device `[PEER KEY NAME PAIRED-AT]`, as Lisp stored it.
fn stored_device(record: Vector<'_>) -> Result<Device> {
    let peer_text: String = record.get(0)?;
    let key_text: String = record.get(1)?;
    let name: String = record.get(2)?;
    let paired_at: i64 = record.get(3)?;
    Ok(Device::new(
        peer(&peer_text)?,
        key(&key_text)?,
        DeviceName::new(&name)?,
        Millis::new(u64::try_from(paired_at)?),
    ))
}

// ----------------------------------------------------------------------
// Values to Lisp

/// The symbol of a fieldless variant: its name in kebab case, so
/// `OfferEnd::Expired` is `expired` and `CloseReason::ByPeer` `by-peer`.
fn variant<'e>(env: &'e Env, value: impl Debug) -> Result<Value<'e>> {
    let name = format!("{value:?}");
    let mut symbol = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                symbol.push('-');
            }
            symbol.push(character.to_ascii_lowercase());
        } else {
            symbol.push(character);
        }
    }
    env.intern(&symbol)
}

fn text(peer: &PeerId) -> String {
    peer.to_string()
}

fn millis(moment: Millis) -> Result<i64> {
    Ok(i64::try_from(moment.get())?)
}

fn lispm_effect(env: &Env, effect: lispm::Effect) -> Result<Value<'_>> {
    let tag = |name: &str| env.intern(name);
    match effect {
        lispm::Effect::Transmit { to, frame } => env.list((tag("transmit")?, text(&to), frame)),
        lispm::Effect::ShowOffer(token) => env.list((tag("show-offer")?, token.expose())),
        lispm::Effect::AskOwner { name, fingerprint } => {
            env.list((tag("ask-owner")?, name.as_str(), fingerprint.to_string()))
        }
        lispm::Effect::OfferEnded(end) => env.list((tag("offer-ended")?, variant(env, end)?)),
        lispm::Effect::Paired(device) => env.list((
            tag("paired")?,
            text(device.peer()),
            hex::encode(device.key().as_bytes()),
            device.name().as_str(),
            millis(device.paired_at())?,
        )),
        lispm::Effect::Forgotten(peer) => env.list((tag("forgotten")?, text(&peer))),
        lispm::Effect::Opened { peer, channel } => {
            env.list((tag("opened")?, text(&peer), hex::encode(channel.as_bytes())))
        }
        lispm::Effect::Delivered {
            peer,
            channel,
            payload,
        } => env.list((
            tag("delivered")?,
            text(&peer),
            hex::encode(channel.as_bytes()),
            payload.into_bytes(),
        )),
        lispm::Effect::Closed { peer, channel, why } => env.list((
            tag("closed")?,
            text(&peer),
            hex::encode(channel.as_bytes()),
            variant(env, why)?,
        )),
        lispm::Effect::Discarded { from, why } => {
            env.list((tag("discarded")?, text(&from), format!("{why:?}")))
        }
    }
}

fn device_effect(env: &Env, effect: device::Effect) -> Result<Value<'_>> {
    let tag = |name: &str| env.intern(name);
    match effect {
        device::Effect::Transmit { to, frame } => env.list((tag("transmit")?, text(&to), frame)),
        device::Effect::ShowFingerprint(fingerprint) => {
            env.list((tag("show-fingerprint")?, fingerprint.to_string()))
        }
        device::Effect::Paired(pairing) => env.list((
            tag("paired")?,
            text(pairing.responder()),
            hex::encode(pairing.key().as_bytes()),
        )),
        device::Effect::Unpaired => env.list((tag("unpaired")?,)),
        device::Effect::Opened(channel) => {
            env.list((tag("opened")?, hex::encode(channel.as_bytes())))
        }
        device::Effect::Delivered(payload) => env.list((tag("delivered")?, payload.into_bytes())),
        device::Effect::Closed(why) => env.list((tag("closed")?, variant(env, why)?)),
        device::Effect::Failed(device::Failure::Refused(refusal)) => {
            env.list((tag("failed")?, tag("refused")?, variant(env, refusal)?))
        }
        device::Effect::Failed(failure) => env.list((tag("failed")?, variant(env, failure)?)),
        device::Effect::Discarded(why) => env.list((tag("discarded")?, format!("{why:?}"))),
    }
}

fn effects<'e, E>(
    env: &'e Env,
    effects: Vec<E>,
    convert: fn(&'e Env, E) -> Result<Value<'e>>,
) -> Result<Value<'e>> {
    let values = effects
        .into_iter()
        .map(|effect| convert(env, effect))
        .collect::<Result<Vec<_>>>()?;
    env.list(&values)
}

// ----------------------------------------------------------------------
// Stepping

/// Steps LispM's machine with `input`.  Post: on error, the machine is
/// the one given.
fn step_lispm<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    input: lispm::Input,
) -> Result<Value<'e>> {
    let state = machine
        .0
        .take()
        .ok_or_else(|| emacs::Error::msg("the responder is stepping"))?;
    let (state, outcome) = state.step(now(), input);
    machine.0 = Some(state);
    effects(env, outcome?, lispm_effect)
}

/// Steps the device's machine with `input`.  Post: on error, the machine
/// is the one given.
fn step_device<'e>(
    env: &'e Env,
    machine: &mut DeviceMachine,
    input: device::Input,
) -> Result<Value<'e>> {
    let state = machine
        .0
        .take()
        .ok_or_else(|| emacs::Error::msg("the initiator is stepping"))?;
    let (state, outcome) = state.step(now(), input);
    machine.0 = Some(state);
    effects(env, outcome?, device_effect)
}

// ----------------------------------------------------------------------
// Keys

/// A fresh static secret, as hexadecimal text, for the caller to keep
/// secret and store.
#[defun]
fn generate_static() -> Result<String> {
    Ok(hex::encode(StaticSecret::generate(entropy()?).expose()))
}

// ----------------------------------------------------------------------
// LispM's side

/// LispM's pairing machine for the static secret SECRET at the DID DID,
/// remembering DEVICES, a list of vectors [PEER KEY NAME PAIRED-AT].
#[defun(user_ptr)]
fn responder(secret: String, did: String, devices: Value<'_>) -> Result<LispmMachine> {
    let env = devices.env;
    let devices: Vector<'_> = env.call("vconcat", (devices,))?.into_rust()?;
    let records = (0..devices.len())
        .map(|index| devices.get::<Vector<'_>>(index).and_then(stored_device))
        .collect::<Result<Vec<_>>>()?;
    Ok(LispmMachine(Some(Responder::new(
        identity(&secret, &did)?,
        records,
    ))))
}

/// Feed MACHINE the FRAME, a unibyte string, that the peer FROM sent.
#[defun]
fn responder_receive<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    from: String,
    frame: Vec<u8>,
) -> Result<Value<'e>> {
    let input = lispm::Input::Receive {
        from: peer(&from)?,
        frame,
        entropy: entropy()?,
    };
    step_lispm(env, machine, input)
}

/// Make MACHINE offer pairing for SECONDS, replacing any other offer.
#[defun]
fn responder_offer<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    seconds: u64,
) -> Result<Value<'e>> {
    let input = lispm::Input::Offer {
        expires: now().after(Duration::from_secs(seconds)),
        entropy: entropy()?,
    };
    step_lispm(env, machine, input)
}

/// Approve the device claiming MACHINE's offer, whose FINGERPRINT the
/// owner compared.
#[defun]
fn responder_approve<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    fingerprint: String,
) -> Result<Value<'e>> {
    let fingerprint: Fingerprint = fingerprint.parse()?;
    step_lispm(env, machine, lispm::Input::Approve { fingerprint })
}

/// Refuse the device claiming MACHINE's offer.
#[defun]
fn responder_refuse<'e>(env: &'e Env, machine: &mut LispmMachine) -> Result<Value<'e>> {
    step_lispm(env, machine, lispm::Input::Refuse)
}

/// Withdraw MACHINE's offer.
#[defun]
fn responder_withdraw<'e>(env: &'e Env, machine: &mut LispmMachine) -> Result<Value<'e>> {
    step_lispm(env, machine, lispm::Input::Withdraw)
}

/// Send PAYLOAD, a unibyte string, on MACHINE's CHANNEL to the device PEER.
#[defun]
fn responder_send<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    peer_text: String,
    channel_text: String,
    payload: Vec<u8>,
) -> Result<Value<'e>> {
    let input = lispm::Input::Send {
        peer: peer(&peer_text)?,
        channel: channel(&channel_text)?,
        payload: Payload::new(payload)?,
    };
    step_lispm(env, machine, input)
}

/// Close MACHINE's CHANNEL to the device PEER.
#[defun]
fn responder_close<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    peer_text: String,
    channel_text: String,
) -> Result<Value<'e>> {
    let input = lispm::Input::Close {
        peer: peer(&peer_text)?,
        channel: channel(&channel_text)?,
    };
    step_lispm(env, machine, input)
}

/// Make MACHINE forget the device PEER and close its channels.
#[defun]
fn responder_forget<'e>(
    env: &'e Env,
    machine: &mut LispmMachine,
    peer_text: String,
) -> Result<Value<'e>> {
    let input = lispm::Input::Forget {
        peer: peer(&peer_text)?,
    };
    step_lispm(env, machine, input)
}

/// Tell MACHINE that time passed.
#[defun]
fn responder_tick<'e>(env: &'e Env, machine: &mut LispmMachine) -> Result<Value<'e>> {
    step_lispm(env, machine, lispm::Input::Tick)
}

// ----------------------------------------------------------------------
// A device's side

/// A device's pairing machine for the static secret SECRET at the DID DID,
/// paired as PAIRING says: nil, or a vector [RESPONDER KEY].
#[defun(user_ptr)]
fn initiator(secret: String, did: String, pairing: Option<Vector<'_>>) -> Result<DeviceMachine> {
    let pairing = match pairing {
        Some(record) => {
            let responder: String = record.get(0)?;
            let key_text: String = record.get(1)?;
            Some(Pairing::new(peer(&responder)?, key(&key_text)?))
        }
        None => None,
    };
    Ok(DeviceMachine(Some(Initiator::new(
        identity(&secret, &did)?,
        pairing,
    ))))
}

/// Pair MACHINE with the offer TOKEN, calling the device NAME.
#[defun]
fn initiator_pair<'e>(
    env: &'e Env,
    machine: &mut DeviceMachine,
    token: String,
    name: String,
) -> Result<Value<'e>> {
    let input = device::Input::Pair {
        offer: Offer::from_token(&token)?,
        name: DeviceName::new(&name)?,
        entropy: entropy()?,
    };
    step_device(env, machine, input)
}

/// Open a session of the paired MACHINE.
#[defun]
fn initiator_connect<'e>(env: &'e Env, machine: &mut DeviceMachine) -> Result<Value<'e>> {
    let input = device::Input::Connect {
        entropy: entropy()?,
    };
    step_device(env, machine, input)
}

/// Feed MACHINE the FRAME, a unibyte string, that the peer FROM sent.
#[defun]
fn initiator_receive<'e>(
    env: &'e Env,
    machine: &mut DeviceMachine,
    from: String,
    frame: Vec<u8>,
) -> Result<Value<'e>> {
    let input = device::Input::Receive {
        from: peer(&from)?,
        frame,
    };
    step_device(env, machine, input)
}

/// Send PAYLOAD, a unibyte string, on MACHINE's open channel.
#[defun]
fn initiator_send<'e>(
    env: &'e Env,
    machine: &mut DeviceMachine,
    payload: Vec<u8>,
) -> Result<Value<'e>> {
    step_device(env, machine, device::Input::Send(Payload::new(payload)?))
}

/// Close MACHINE's channel, or stop opening it.
#[defun]
fn initiator_close<'e>(env: &'e Env, machine: &mut DeviceMachine) -> Result<Value<'e>> {
    step_device(env, machine, device::Input::Close)
}

/// Tell MACHINE that time passed.
#[defun]
fn initiator_tick<'e>(env: &'e Env, machine: &mut DeviceMachine) -> Result<Value<'e>> {
    step_device(env, machine, device::Input::Tick)
}

/// Whether MACHINE has an open channel.
#[defun]
fn initiator_open_p(machine: &DeviceMachine) -> Result<bool> {
    Ok(machine.0.as_ref().is_some_and(Initiator::is_open))
}

// ----------------------------------------------------------------------
// QR codes

/// The QR code of TEXT: a list of rows, each a string of 1 (dark) and 0
/// (light), without the quiet zone.
#[defun]
fn qr(env: &Env, text: String) -> Result<Value<'_>> {
    let code = qrcode::QrCode::new(text.as_bytes())?;
    let width = code.width();
    let rows = code
        .to_colors()
        .chunks(width.max(1))
        .map(|row| {
            row.iter()
                .map(|color| match color {
                    qrcode::Color::Dark => '1',
                    qrcode::Color::Light => '0',
                })
                .collect::<String>()
                .into_lisp(env)
        })
        .collect::<Result<Vec<_>>>()?;
    env.list(&rows)
}
