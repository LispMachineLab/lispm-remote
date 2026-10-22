// lispm-remote: randomness as an argument.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Randomness enters the state machines as a value, [`Entropy`], which the
//! shell draws from its cryptographic random source.  Every secret the
//! protocol needs is derived from one such value by a pseudo-random
//! function indexed by its [`Purpose`]:
//!
//! ```text
//!   derive(e, p) = BLAKE2s-MAC(key = e, message = label(p))
//!   label injective  ⇒  derive(e, p) and derive(e, q) independent for p ≠ q
//! ```
//!
//! Snow draws its ephemeral keys from the [`SeededResolver`]'s generator, a
//! ChaCha20 keystream under `derive(e, Handshake)`.  The resolver hands out
//! that generator once, so no two handshakes, nor two calls in one, share a
//! keystream.

use core::cell::Cell;
use core::fmt;

use blake2::Blake2sMac256;
use blake2::digest::KeyInit;
use blake2::digest::Mac;
use chacha20::ChaCha20;
use chacha20::cipher::KeyIvInit;
use chacha20::cipher::StreamCipher;
use snow::params::CipherChoice;
use snow::params::DHChoice;
use snow::params::HashChoice;
use snow::resolvers::CryptoResolver;
use snow::resolvers::DefaultResolver;
use snow::types::Cipher;
use snow::types::Dh;
use snow::types::Hash;
use snow::types::Random;
use zeroize::Zeroize;
use zeroize::ZeroizeOnDrop;

/// The length of an [`Entropy`] value and of every derived secret.
pub const ENTROPY_LEN: usize = 32;

/// Thirty-two bytes from the shell's cryptographic random source.
///
/// A value is consumed by the step it is given to and never copied, so one
/// draw feeds one transition.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Entropy([u8; ENTROPY_LEN]);

impl Entropy {
    /// Wraps bytes the shell drew from a cryptographic random source.
    #[must_use]
    pub const fn new(bytes: [u8; ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    /// The secret derived from this value for `purpose`.
    pub(crate) fn derive(&self, purpose: Purpose) -> Derived {
        Derived(mac(&self.0, purpose.label()))
    }
}

impl fmt::Debug for Entropy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Entropy(..)")
    }
}

/// What a derived secret is for.  Each purpose has its own label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The identifier of a channel or an offer.
    Channel,
    /// The pre-shared key of an offer.
    Psk,
    /// The seed of a handshake's generator.
    Handshake,
    /// A static secret key.
    Static,
}

impl Purpose {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Channel => b"lispm-remote/1 channel",
            Self::Psk => b"lispm-remote/1 psk",
            Self::Handshake => b"lispm-remote/1 handshake",
            Self::Static => b"lispm-remote/1 static",
        }
    }
}

/// A secret derived from [`Entropy`] for one [`Purpose`].
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct Derived([u8; ENTROPY_LEN]);

impl Derived {
    /// The derived bytes.
    pub(crate) const fn bytes(&self) -> &[u8; ENTROPY_LEN] {
        &self.0
    }
}

fn mac(key: &[u8; ENTROPY_LEN], message: &[u8]) -> [u8; ENTROPY_LEN] {
    let mut mac = <Blake2sMac256 as KeyInit>::new(key.into());
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// A ChaCha20 keystream: the generator of one handshake.
struct Keystream(ChaCha20);

impl Random for Keystream {
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), snow::Error> {
        dest.fill(0);
        self.0
            .try_apply_keystream(dest)
            .map_err(|_| snow::Error::Rng)
    }
}

/// Snow's default primitives, with randomness from a seed given once.
pub(crate) struct SeededResolver {
    seed: Cell<Option<Derived>>,
}

impl SeededResolver {
    /// A resolver whose generator is seeded from `entropy`.
    pub(crate) fn new(entropy: &Entropy) -> Self {
        Self {
            seed: Cell::new(Some(entropy.derive(Purpose::Handshake))),
        }
    }
}

impl CryptoResolver for SeededResolver {
    // Post: the first call returns the keystream; every later call None.
    fn resolve_rng(&self) -> Option<Box<dyn Random>> {
        let seed = self.seed.take()?;
        let nonce = [0_u8; 12];
        let cipher = ChaCha20::new(seed.bytes().into(), &nonce.into());
        Some(Box::new(Keystream(cipher)))
    }

    fn resolve_dh(&self, choice: &DHChoice) -> Option<Box<dyn Dh>> {
        DefaultResolver.resolve_dh(choice)
    }

    fn resolve_hash(&self, choice: &HashChoice) -> Option<Box<dyn Hash>> {
        DefaultResolver.resolve_hash(choice)
    }

    fn resolve_cipher(&self, choice: &CipherChoice) -> Option<Box<dyn Cipher>> {
        DefaultResolver.resolve_cipher(choice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PURPOSES: [Purpose; 4] = [
        Purpose::Channel,
        Purpose::Psk,
        Purpose::Handshake,
        Purpose::Static,
    ];

    #[test]
    fn derivation_is_a_function_of_entropy_and_purpose() {
        let a = Entropy::new([7; ENTROPY_LEN]);
        let b = Entropy::new([7; ENTROPY_LEN]);
        for purpose in PURPOSES {
            assert_eq!(a.derive(purpose).bytes(), b.derive(purpose).bytes());
        }
    }

    #[test]
    fn distinct_purposes_derive_distinct_secrets() {
        let entropy = Entropy::new([9; ENTROPY_LEN]);
        for (i, p) in PURPOSES.iter().enumerate() {
            for q in PURPOSES.iter().skip(i + 1) {
                assert_ne!(entropy.derive(*p).bytes(), entropy.derive(*q).bytes());
            }
        }
    }

    #[test]
    fn the_resolver_hands_out_its_generator_once() {
        let resolver = SeededResolver::new(&Entropy::new([1; ENTROPY_LEN]));
        assert!(resolver.resolve_rng().is_some());
        assert!(resolver.resolve_rng().is_none());
    }

    #[test]
    fn equal_seeds_give_equal_keystreams() {
        let draw = || {
            let resolver = SeededResolver::new(&Entropy::new([3; ENTROPY_LEN]));
            let mut rng = resolver.resolve_rng().unwrap();
            let mut out = [0_u8; 64];
            rng.try_fill_bytes(&mut out).unwrap();
            out
        };
        assert_eq!(draw(), draw());
    }
}
