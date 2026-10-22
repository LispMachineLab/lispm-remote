// lispm-remote: the two Noise handshakes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two patterns, both two messages long, with the device as initiator `I`
//! and LispM as responder `R`:
//!
//! ```text
//!   Pairing  Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s    (I knows R's s from the offer)
//!     <- s
//!     -> e, es, s, ss, psk       payload: the device's name
//!     <- e, ee, se               sent only once the owner approves
//!
//!   Session  Noise_KK_25519_ChaChaPoly_BLAKE2s        (each knows the other's s)
//!     -> s
//!     <- s
//!     -> e, es, ss
//!     <- e, ee, se
//! ```
//!
//! With `psk1` the pre-shared key is mixed into the first message, so the
//! responder learns that the device read the offer before it answers.  The
//! prologue binds each handshake to its stage, both transport addresses and
//! the channel, as an injective encoding:
//!
//! ```text
//!   prologue = "lispm-remote/1" ‖ stage:u8 ‖ len(I) ‖ I ‖ len(R) ‖ R ‖ channel
//! ```
//!
//! A message relayed under another address, on another channel or at the
//! other stage therefore fails to decrypt.

use snow::HandshakeState;

use crate::channel::Channel;
use crate::entropy::Entropy;
use crate::entropy::SeededResolver;
use crate::ids::ChannelId;
use crate::ids::PeerId;
use crate::keys::Fingerprint;
use crate::keys::Identity;
use crate::keys::Psk;
use crate::keys::PublicKey;
use crate::wire::MAX_NOISE_LEN;

const PROLOGUE_LABEL: &[u8] = b"lispm-remote/1";
const PSK_LOCATION: u8 = 1;

/// Which handshake: pairing on an offer, or a session of a paired device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Pair,
    Open,
}

impl Stage {
    const fn pattern(self) -> &'static str {
        match self {
            Self::Pair => "Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s",
            Self::Open => "Noise_KK_25519_ChaChaPoly_BLAKE2s",
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::Pair => 1,
            Self::Open => 2,
        }
    }
}

/// Which end builds the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Initiator,
    Responder,
}

/// Everything a handshake is bound to.
pub(crate) struct Transcript<'a> {
    pub(crate) stage: Stage,
    pub(crate) initiator: &'a PeerId,
    pub(crate) responder: &'a PeerId,
    pub(crate) channel: ChannelId,
}

impl Transcript<'_> {
    fn prologue(&self) -> Vec<u8> {
        let mut bytes = PROLOGUE_LABEL.to_vec();
        bytes.push(self.stage.tag());
        for peer in [self.initiator, self.responder] {
            bytes.push(peer.len_byte());
            bytes.extend_from_slice(peer.as_bytes());
        }
        bytes.extend_from_slice(self.channel.as_bytes());
        bytes
    }
}

/// One end's half of a handshake, before it is built.
///
/// Pre: `remote` is `Some` unless `role = Responder ∧ stage = Pair`, and
/// `psk` is `Some` exactly when `stage = Pair`.
pub(crate) struct Spec<'a> {
    pub(crate) role: Role,
    pub(crate) transcript: Transcript<'a>,
    pub(crate) local: &'a Identity,
    pub(crate) remote: Option<&'a PublicKey>,
    pub(crate) psk: Option<&'a Psk>,
}

impl Spec<'_> {
    /// The handshake this spec describes, its generator seeded from
    /// `entropy`.
    pub(crate) fn build(self, entropy: &Entropy) -> Result<HandshakeState, HandshakeError> {
        let params = self
            .transcript
            .stage
            .pattern()
            .parse()
            .map_err(|_| HandshakeError::Setup)?;
        let prologue = self.transcript.prologue();
        let resolver = Box::new(SeededResolver::new(entropy));
        let mut builder = snow::Builder::with_resolver(params, resolver)
            .local_private_key(self.local.secret().expose())
            .and_then(|b| b.prologue(&prologue))
            .map_err(|_| HandshakeError::Setup)?;
        if let Some(remote) = self.remote {
            builder = builder
                .remote_public_key(remote.as_bytes())
                .map_err(|_| HandshakeError::Setup)?;
        }
        if let Some(psk) = self.psk {
            builder = builder
                .psk(PSK_LOCATION, psk.as_bytes())
                .map_err(|_| HandshakeError::Setup)?;
        }
        match self.role {
            Role::Initiator => builder.build_initiator(),
            Role::Responder => builder.build_responder(),
        }
        .map_err(|_| HandshakeError::Setup)
    }
}

/// The next message of `handshake`, carrying `payload`.
pub(crate) fn write(
    handshake: &mut HandshakeState,
    payload: &[u8],
) -> Result<Vec<u8>, HandshakeError> {
    let mut message = vec![0_u8; MAX_NOISE_LEN];
    let len = handshake
        .write_message(payload, &mut message)
        .map_err(|_| HandshakeError::Setup)?;
    message.truncate(len);
    Ok(message)
}

/// The payload of `message`, read as `handshake`'s next message.
pub(crate) fn read(
    handshake: &mut HandshakeState,
    message: &[u8],
) -> Result<Vec<u8>, HandshakeError> {
    let mut payload = vec![0_u8; MAX_NOISE_LEN];
    let len = handshake
        .read_message(message, &mut payload)
        .map_err(|_| HandshakeError::Rejected)?;
    payload.truncate(len);
    Ok(payload)
}

/// The static key the other end proved, once the handshake learned it.
pub(crate) fn remote_static(handshake: &HandshakeState) -> Option<PublicKey> {
    handshake
        .get_remote_static()
        .and_then(PublicKey::from_slice)
}

/// The fingerprint both ends compute after the first message.
pub(crate) fn fingerprint(handshake: &HandshakeState) -> Fingerprint {
    Fingerprint::of_handshake(handshake.get_handshake_hash())
}

/// The channel a finished handshake opens.
pub(crate) fn finish(handshake: HandshakeState) -> Result<Channel, HandshakeError> {
    handshake
        .into_stateless_transport_mode()
        .map(Channel::new)
        .map_err(|_| HandshakeError::Setup)
}

/// Why a handshake did not go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// The other end's message did not authenticate: wrong keys, wrong
    /// pre-shared key, another prologue, or tampering.
    #[error("the handshake message did not authenticate")]
    Rejected,
    /// This end could not build or advance its own handshake.
    #[error("the handshake could not be set up")]
    Setup,
}
