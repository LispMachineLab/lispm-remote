// lispm-remote: an open session.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A channel is a finished handshake: two ChaCha20-Poly1305 keys, one per
//! direction.  Sending seals each body under the next counter; receiving
//! opens a record only if its nonce is in the [`ReplayWindow`] and its tag
//! verifies, and only then marks the nonce:
//!
//! ```text
//!   seal:  next < 2⁶⁴ − 1  ⇒  (next, AEAD_k(next, body)),  next ← next + 1
//!   open:  window.accepts(n) ∧ AEAD⁻¹_k'(n, c) = body  ⇒  window ← mark(n)
//! ```
//!
//! Marking after verification means a forged record cannot use up a
//! nonce the real one needs.

use snow::StatelessTransportState;

use crate::replay::ReplayWindow;
use crate::wire::Body;
use crate::wire::MAX_NOISE_LEN;
use crate::wire::Nonce;
use crate::wire::WireError;

/// The keys and counters of one open session.
pub(crate) struct Channel {
    transport: StatelessTransportState,
    next: u64,
    window: ReplayWindow,
}

impl Channel {
    pub(crate) fn new(transport: StatelessTransportState) -> Self {
        Self {
            transport,
            next: 0,
            window: ReplayWindow::default(),
        }
    }

    /// `body` sealed under the next nonce.
    pub(crate) fn seal(&mut self, body: &Body) -> Result<(Nonce, Vec<u8>), SealError> {
        let nonce = self.next;
        let next = nonce.checked_add(1).ok_or(SealError::Exhausted)?;
        let mut sealed = vec![0_u8; MAX_NOISE_LEN];
        let len = self
            .transport
            .write_message(nonce, &body.encode(), &mut sealed)
            .map_err(|_| SealError::Oversized)?;
        sealed.truncate(len);
        self.next = next;
        Ok((Nonce::new(nonce), sealed))
    }

    /// The body of the record `sealed` under `nonce`.
    pub(crate) fn open(&mut self, nonce: Nonce, sealed: &[u8]) -> Result<Body, OpenError> {
        if !self.window.accepts(nonce) {
            return Err(OpenError::Replayed);
        }
        let mut plain = vec![0_u8; MAX_NOISE_LEN];
        let len = self
            .transport
            .read_message(nonce.get(), sealed, &mut plain)
            .map_err(|_| OpenError::Forged)?;
        plain.truncate(len);
        let body = Body::decode(&plain).map_err(OpenError::Malformed)?;
        self.window = self.window.mark(nonce);
        Ok(body)
    }
}

/// Why a body could not be sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    /// The channel used up its nonces; open another.
    #[error("the channel has used up its nonces")]
    Exhausted,
    /// The body does not fit one Noise message.
    #[error("the body does not fit one record")]
    Oversized,
}

/// Why a record was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// Its nonce was accepted before, or is too old to tell.
    #[error("the record was replayed")]
    Replayed,
    /// Its tag did not verify.
    #[error("the record did not authenticate")]
    Forged,
    /// It authenticated but holds no body.
    #[error("the record holds no body: {0}")]
    Malformed(WireError),
}
