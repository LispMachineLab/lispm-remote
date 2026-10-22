// lispm-remote: what LispM's side takes, does and refuses.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The responder's inputs, effects, records and errors.

use crate::channel::OpenError;
use crate::channel::SealError;
use crate::clock::Millis;
use crate::entropy::Entropy;
use crate::ids::ChannelId;
use crate::ids::DeviceName;
use crate::ids::IdError;
use crate::ids::PeerId;
use crate::keys::Fingerprint;
use crate::keys::PublicKey;
use crate::noise::HandshakeError;
use crate::offer::OfferToken;
use crate::wire::Payload;
use crate::wire::WireError;

/// A paired device, as the shell stores it.
///
/// Law: `Clone` is sound because a record is data; the responder keeps one
/// copy and hands the shell another to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub(super) peer: PeerId,
    pub(super) key: PublicKey,
    pub(super) name: DeviceName,
    pub(super) paired_at: Millis,
}

impl Device {
    /// The record of the device at `peer`, keyed `key`, named `name`,
    /// paired at `paired_at`, as the shell restores it.
    #[must_use]
    pub const fn new(peer: PeerId, key: PublicKey, name: DeviceName, paired_at: Millis) -> Self {
        Self {
            peer,
            key,
            name,
            paired_at,
        }
    }

    /// The device's transport address.
    #[must_use]
    pub const fn peer(&self) -> &PeerId {
        &self.peer
    }

    /// The device's static public key.
    #[must_use]
    pub const fn key(&self) -> &PublicKey {
        &self.key
    }

    /// What the device called itself.
    #[must_use]
    pub const fn name(&self) -> &DeviceName {
        &self.name
    }

    /// When it was paired.
    #[must_use]
    pub const fn paired_at(&self) -> Millis {
        self.paired_at
    }
}

/// Something the responder needs or does.
#[derive(Debug)]
pub enum Input {
    /// A frame arrived from `from`, as the transport authenticated it.
    Receive {
        /// The sender.
        from: PeerId,
        /// The frame's bytes.
        frame: Vec<u8>,
        /// Fresh randomness, for a handshake the frame may start.
        entropy: Entropy,
    },
    /// Offer pairing until `expires`, replacing any other offer.
    Offer {
        /// The end of the offer.
        expires: Millis,
        /// Fresh randomness for its channel and key.
        entropy: Entropy,
    },
    /// The owner lets the claimant pair, having compared `fingerprint`.
    Approve {
        /// The fingerprint the owner saw.
        fingerprint: Fingerprint,
    },
    /// The owner turns the claimant away.
    Refuse,
    /// The owner takes the offer back.
    Withdraw,
    /// Send `payload` on a channel.
    Send {
        /// The device.
        peer: PeerId,
        /// The channel.
        channel: ChannelId,
        /// The bytes.
        payload: Payload,
    },
    /// Close a channel.
    Close {
        /// The device.
        peer: PeerId,
        /// The channel.
        channel: ChannelId,
    },
    /// Unpair a device and close its channels.
    Forget {
        /// The device.
        peer: PeerId,
    },
    /// Time passed.
    Tick,
}

/// Something the shell must do or show.
#[derive(Debug)]
pub enum Effect {
    /// Send `frame` to `to`.
    Transmit {
        /// The receiver.
        to: PeerId,
        /// The frame's bytes.
        frame: Vec<u8>,
    },
    /// Show the owner this token as a QR code.
    ShowOffer(OfferToken),
    /// Ask the owner whether the device `name`, which shows `fingerprint`,
    /// may pair.  A later question replaces an earlier one.
    AskOwner {
        /// What the device called itself.
        name: DeviceName,
        /// The fingerprint the device shows.
        fingerprint: Fingerprint,
    },
    /// The offer ended: take its QR code and question away.
    OfferEnded(OfferEnd),
    /// Store this device.
    Paired(Device),
    /// Delete the stored device at this address.
    Forgotten(PeerId),
    /// A channel opened.
    Opened {
        /// The device.
        peer: PeerId,
        /// The channel.
        channel: ChannelId,
    },
    /// A channel delivered a payload.
    Delivered {
        /// The device.
        peer: PeerId,
        /// The channel.
        channel: ChannelId,
        /// The bytes.
        payload: Payload,
    },
    /// A channel closed.
    Closed {
        /// The device.
        peer: PeerId,
        /// The channel.
        channel: ChannelId,
        /// Why.
        why: CloseReason,
    },
    /// A frame was discarded.
    Discarded {
        /// Its sender.
        from: PeerId,
        /// Why.
        why: Discard,
    },
}

/// How an offer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferEnd {
    /// A device paired with it.
    Used,
    /// Its time ran out.
    Expired,
    /// The owner refused the claimant.
    Refused,
    /// Two devices proved its key.
    Contested,
    /// The owner withdrew it.
    Withdrawn,
    /// A newer offer replaced it.
    Replaced,
    /// This end could not finish the handshake.
    Failed,
}

/// Why a channel closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The device closed it.
    ByPeer,
    /// This end closed it.
    ByUs,
    /// The device opened more channels than it may keep.
    Evicted,
    /// The device was forgotten.
    Forgotten,
    /// The device paired again.
    Repaired,
}

/// Why a frame was discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discard {
    /// It is not a frame.
    Malformed(WireError),
    /// It is a kind this end never receives.
    Unexpected,
    /// It claims an offer this end does not hold.
    NoOffer,
    /// It claims an offer that expired.
    Expired,
    /// Its handshake failed.
    Handshake(HandshakeError),
    /// Its device name is invalid.
    Name(IdError),
    /// Its sender is not paired.
    NotPaired,
    /// It opens a channel that is already open.
    Duplicate,
    /// It names a channel that is not open.
    UnknownChannel,
    /// Its record was not accepted.
    Record(OpenError),
}

/// Why a command was refused.  The state is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// There is no offer.
    #[error("there is no offer")]
    NoOffer,
    /// No device has claimed the offer.
    #[error("no device has claimed the offer")]
    NoClaim,
    /// The claim changed since the owner saw it.
    #[error("the claim changed since the owner saw it")]
    StaleApproval,
    /// The offer has expired.
    #[error("the offer has expired")]
    Expired,
    /// The offer would expire before it starts.
    #[error("the offer would expire at once")]
    InvalidExpiry,
    /// Every device slot is taken.
    #[error("every device slot is taken")]
    TooManyDevices,
    /// No such device.
    #[error("no such device")]
    NoSuchDevice,
    /// No such channel.
    #[error("no such channel")]
    NoSuchChannel,
    /// The payload could not be sealed.
    #[error("the payload could not be sealed: {0}")]
    Seal(SealError),
}
